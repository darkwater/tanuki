use std::{collections::VecDeque, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
/// A validated topic path.
///
/// Fields are private so external input must pass through validation.
///
/// ```compile_fail
/// use tanuki::domain::TopicPath;
///
/// let path = TopicPath { segments: vec!["*".to_owned()].into_boxed_slice() };
/// ```
pub struct TopicPath {
    segments: Box<[String]>,
}

impl TopicPath {
    pub fn parse(input: &str) -> Result<Self, PathParseError> {
        let segments = parse_raw_segments(input, RootPolicy::Reject)?;
        for (index, segment) in segments.iter().enumerate() {
            validate_topic_segment(index, segment)?;
        }
        Ok(Self {
            segments: segments.into_boxed_slice(),
        })
    }

    pub fn parse_ordinary(input: &str) -> Result<Self, PathParseError> {
        let path = Self::parse(input)?;
        if let Some(segment) = path
            .segments
            .first()
            .filter(|segment| segment.starts_with('$'))
        {
            return Err(PathParseError::ReservedSystemNamespace {
                segment: segment.clone(),
            });
        }
        Ok(path)
    }

    #[must_use]
    pub fn is_system(&self) -> bool {
        self.segments
            .first()
            .is_some_and(|segment| segment.starts_with('$'))
    }

    pub fn segments(&self) -> impl ExactSizeIterator<Item = &str> {
        self.segments.iter().map(String::as_str)
    }
}

impl fmt::Display for TopicPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "/{}", self.segments.join("/"))
    }
}

impl FromStr for TopicPath {
    type Err = PathParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl TryFrom<&str> for TopicPath {
    type Error = PathParseError;

    fn try_from(input: &str) -> Result<Self, Self::Error> {
        Self::parse(input)
    }
}

impl Serialize for TopicPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TopicPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = String::deserialize(deserializer)?;
        Self::parse(&input).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selector {
    segments: Box<[SelectorSegment]>,
}

impl Selector {
    pub fn parse(input: &str) -> Result<Self, SelectorParseError> {
        let raw_segments = parse_raw_segments(input, RootPolicy::Allow)?;
        let segments = raw_segments
            .iter()
            .enumerate()
            .map(|(index, segment)| parse_selector_segment(index, segment))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            segments: segments.into_boxed_slice(),
        })
    }

    #[must_use]
    pub fn matches(&self, topic: &TopicPath) -> bool {
        let pattern_len = self.segments.len();
        let topic_len = topic.segments.len();
        let mut matched = vec![vec![false; topic_len + 1]; pattern_len + 1];
        matched[pattern_len][topic_len] = true;

        for pattern_index in (0..pattern_len).rev() {
            for topic_index in (0..=topic_len).rev() {
                matched[pattern_index][topic_index] = match &self.segments[pattern_index] {
                    SelectorSegment::Recursive => {
                        matched[pattern_index + 1][topic_index]
                            || (topic_index < topic_len && matched[pattern_index][topic_index + 1])
                    }
                    segment if topic_index < topic_len => {
                        segment.matches(&topic.segments[topic_index])
                            && matched[pattern_index + 1][topic_index + 1]
                    }
                    _ => false,
                };
            }
        }

        matched[0][0]
    }

    /// Returns whether some valid topic can match both selectors.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        let left_len = self.segments.len();
        let right_len = other.segments.len();
        let mut pending = VecDeque::from([(0, 0, false)]);
        let mut visited = vec![vec![[false; 2]; right_len + 1]; left_len + 1];

        while let Some((left, right, consumed)) = pending.pop_front() {
            let consumed_index = usize::from(consumed);
            if visited[left][right][consumed_index] {
                continue;
            }
            visited[left][right][consumed_index] = true;
            if left == left_len && right == right_len {
                if consumed {
                    return true;
                }
                continue;
            }

            let left_segment = self.segments.get(left);
            let right_segment = other.segments.get(right);
            if matches!(left_segment, Some(SelectorSegment::Recursive)) {
                pending.push_back((left + 1, right, consumed));
            }
            if matches!(right_segment, Some(SelectorSegment::Recursive)) {
                pending.push_back((left, right + 1, consumed));
            }

            match (left_segment, right_segment) {
                (Some(SelectorSegment::Recursive), Some(SelectorSegment::Recursive)) => {
                    pending.push_back((left, right, true));
                }
                (Some(SelectorSegment::Recursive), Some(_)) => {
                    pending.push_back((left, right + 1, true));
                }
                (Some(_), Some(SelectorSegment::Recursive)) => {
                    pending.push_back((left + 1, right, true));
                }
                (Some(left_segment), Some(right_segment))
                    if left_segment.intersects(right_segment) =>
                {
                    pending.push_back((left + 1, right + 1, true));
                }
                _ => {}
            }
        }
        false
    }

    /// Returns whether the selector names a reserved system branch explicitly.
    ///
    /// Broad wildcards are not explicit: schema matching may restrict them to
    /// the ordinary-topic domain without making an ordinary catch-all invalid.
    #[must_use]
    pub fn explicitly_targets_system(&self) -> bool {
        self.segments
            .first()
            .is_some_and(SelectorSegment::explicitly_targets_system)
    }
}

impl FromStr for Selector {
    type Err = SelectorParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl Serialize for Selector {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Selector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = String::deserialize(deserializer)?;
        Self::parse(&input).map_err(de::Error::custom)
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.segments.is_empty() {
            return formatter.write_str("/");
        }

        formatter.write_str("/")?;
        for (index, segment) in self.segments.iter().enumerate() {
            if index > 0 {
                formatter.write_str("/")?;
            }
            segment.fmt(formatter)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Selection {
    selectors: Vec<Selector>,
}

impl Selection {
    #[must_use]
    pub fn new(selectors: Vec<Selector>) -> Self {
        Self { selectors }
    }

    #[must_use]
    pub fn matches(&self, topic: &TopicPath) -> bool {
        self.selectors
            .iter()
            .any(|selector| selector.matches(topic))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selectors.is_empty()
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum PathParseError {
    #[error("a topic path must be absolute")]
    NotAbsolute,
    #[error("the virtual root is not a writable topic")]
    Root,
    #[error("a topic path may not have a trailing slash")]
    TrailingSlash,
    #[error("path segment {index} is empty")]
    EmptySegment { index: usize },
    #[error("path segment {index} may not be `{value}`")]
    DotSegment { index: usize, value: String },
    #[error("path segment {index} contains reserved character `{character}`")]
    ReservedCharacter { index: usize, character: char },
    #[error("path segment {index} contains a control character")]
    ControlCharacter { index: usize },
    #[error("the first segment `{segment}` belongs to the reserved system namespace")]
    ReservedSystemNamespace { segment: String },
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SelectorParseError {
    #[error(transparent)]
    Path(#[from] PathParseError),
    #[error("invalid selector segment {index}: {reason}")]
    InvalidSegment { index: usize, reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SelectorSegment {
    Literal(String),
    One,
    Recursive,
    Choice(Box<[String]>),
}

impl SelectorSegment {
    fn matches(&self, topic_segment: &str) -> bool {
        match self {
            Self::Literal(literal) => literal == topic_segment,
            Self::One => true,
            Self::Recursive => unreachable!("recursive selectors are handled by the matcher"),
            Self::Choice(choices) => choices.iter().any(|choice| choice == topic_segment),
        }
    }

    fn intersects(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::One, _) | (_, Self::One) => true,
            (Self::Literal(left), Self::Literal(right)) => left == right,
            (Self::Literal(literal), Self::Choice(choices))
            | (Self::Choice(choices), Self::Literal(literal)) => choices.contains(literal),
            (Self::Choice(left), Self::Choice(right)) => {
                left.iter().any(|choice| right.contains(choice))
            }
            (Self::Recursive, _) | (_, Self::Recursive) => {
                unreachable!("recursive selector segments are handled by Selector::intersects")
            }
        }
    }

    fn explicitly_targets_system(&self) -> bool {
        match self {
            Self::Literal(value) => value.starts_with('$'),
            Self::Choice(values) => values.iter().any(|value| value.starts_with('$')),
            Self::One | Self::Recursive => false,
        }
    }
}

impl fmt::Display for SelectorSegment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(literal) => formatter.write_str(literal),
            Self::One => formatter.write_str("*"),
            Self::Recursive => formatter.write_str("**"),
            Self::Choice(choices) => write!(formatter, "{{{}}}", choices.join(",")),
        }
    }
}

#[derive(Clone, Copy)]
enum RootPolicy {
    Allow,
    Reject,
}

fn parse_raw_segments(input: &str, root: RootPolicy) -> Result<Vec<String>, PathParseError> {
    if !input.starts_with('/') {
        return Err(PathParseError::NotAbsolute);
    }
    if input == "/" {
        return match root {
            RootPolicy::Allow => Ok(Vec::new()),
            RootPolicy::Reject => Err(PathParseError::Root),
        };
    }
    if input.ends_with('/') {
        return Err(PathParseError::TrailingSlash);
    }

    input[1..]
        .split('/')
        .enumerate()
        .map(|(index, segment)| {
            if segment.is_empty() {
                Err(PathParseError::EmptySegment { index })
            } else if matches!(segment, "." | "..") {
                Err(PathParseError::DotSegment {
                    index,
                    value: segment.to_owned(),
                })
            } else {
                Ok(segment.to_owned())
            }
        })
        .collect()
}

fn validate_topic_segment(index: usize, segment: &str) -> Result<(), PathParseError> {
    if segment.chars().any(char::is_control) {
        return Err(PathParseError::ControlCharacter { index });
    }
    if let Some(character) = segment.chars().find(|character| is_reserved(*character)) {
        return Err(PathParseError::ReservedCharacter { index, character });
    }
    Ok(())
}

fn parse_selector_segment(
    index: usize,
    segment: &str,
) -> Result<SelectorSegment, SelectorParseError> {
    match segment {
        "*" => return Ok(SelectorSegment::One),
        "**" => return Ok(SelectorSegment::Recursive),
        _ => {}
    }

    if segment.starts_with('{') && segment.ends_with('}') {
        let body = &segment[1..segment.len() - 1];
        let choices = body.split(',').map(str::to_owned).collect::<Vec<String>>();
        if choices.len() < 2 || choices.iter().any(String::is_empty) {
            return Err(SelectorParseError::InvalidSegment {
                index,
                reason: "a brace choice needs at least two nonempty alternatives".to_owned(),
            });
        }
        for choice in &choices {
            validate_topic_segment(index, choice).map_err(SelectorParseError::Path)?;
        }
        return Ok(SelectorSegment::Choice(choices.into_boxed_slice()));
    }

    validate_topic_segment(index, segment).map_err(|error| SelectorParseError::InvalidSegment {
        index,
        reason: error.to_string(),
    })?;
    Ok(SelectorSegment::Literal(segment.to_owned()))
}

fn is_reserved(character: char) -> bool {
    matches!(character, '*' | '?' | '[' | ']' | '{' | '}' | '\\')
}
