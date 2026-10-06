use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    str::FromStr,
};

use thiserror::Error;

use crate::domain::{FiniteF64, NodeKind, Selector, TopicPath, Value, ValueKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Enforcement {
    Warn,
    Deny,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NullPolicy {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SchemaName(String);

impl SchemaName {
    pub fn parse(input: &str) -> Result<Self, SchemaNameParseError> {
        if input.is_empty() {
            return Err(SchemaNameParseError::Empty);
        }
        if input.chars().any(char::is_control) {
            return Err(SchemaNameParseError::ControlCharacter);
        }
        if let Some(character) = input
            .chars()
            .find(|character| matches!(character, '/' | '*' | '?' | '[' | ']' | '{' | '}' | '\\'))
        {
            return Err(SchemaNameParseError::ReservedCharacter(character));
        }
        Ok(Self(input.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SchemaName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for SchemaName {
    type Err = SchemaNameParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SchemaNameParseError {
    #[error("a schema name may not be empty")]
    Empty,
    #[error("a schema name may not contain control characters")]
    ControlCharacter,
    #[error("a schema name contains reserved character `{0}`")]
    ReservedCharacter(char),
}

#[derive(Clone, Debug)]
pub struct ValueValidator {
    null_policy: NullPolicy,
    requirement: Requirement,
}

#[derive(Clone, Debug)]
enum Requirement {
    Any,
    Kind(ValueKind),
    IntegerRange {
        minimum: i64,
        maximum: i64,
    },
    FloatRange {
        minimum: FiniteF64,
        maximum: FiniteF64,
    },
    StringEnum(BTreeSet<String>),
}

impl ValueValidator {
    #[must_use]
    pub fn any(null_policy: NullPolicy) -> Self {
        Self {
            null_policy,
            requirement: Requirement::Any,
        }
    }

    pub fn kind(kind: ValueKind, null_policy: NullPolicy) -> Result<Self, ValidatorBuildError> {
        if kind == ValueKind::Null {
            return Err(ValidatorBuildError::NullMustUsePolicy);
        }
        Ok(Self {
            null_policy,
            requirement: Requirement::Kind(kind),
        })
    }

    pub fn integer_range(
        minimum: i64,
        maximum: i64,
        null_policy: NullPolicy,
    ) -> Result<Self, ValidatorBuildError> {
        if minimum > maximum {
            return Err(ValidatorBuildError::ReversedIntegerRange { minimum, maximum });
        }
        Ok(Self {
            null_policy,
            requirement: Requirement::IntegerRange { minimum, maximum },
        })
    }

    pub fn float_range(
        minimum: FiniteF64,
        maximum: FiniteF64,
        null_policy: NullPolicy,
    ) -> Result<Self, ValidatorBuildError> {
        if minimum > maximum {
            return Err(ValidatorBuildError::ReversedFloatRange {
                minimum: minimum.get(),
                maximum: maximum.get(),
            });
        }
        Ok(Self {
            null_policy,
            requirement: Requirement::FloatRange { minimum, maximum },
        })
    }

    pub fn string_enum(
        values: impl IntoIterator<Item = String>,
        null_policy: NullPolicy,
    ) -> Result<Self, ValidatorBuildError> {
        let values = values.into_iter().collect::<BTreeSet<_>>();
        if values.is_empty() {
            return Err(ValidatorBuildError::EmptyStringEnum);
        }
        Ok(Self {
            null_policy,
            requirement: Requirement::StringEnum(values),
        })
    }

    pub fn validate(&self, value: &Value) -> Result<(), ViolationKind> {
        if matches!(value, Value::Null) {
            return match self.null_policy {
                NullPolicy::Allow => Ok(()),
                NullPolicy::Deny => Err(ViolationKind::NullNotAllowed),
            };
        }

        match (&self.requirement, value) {
            (Requirement::Any, _) => Ok(()),
            (Requirement::Kind(expected), value) if *expected == value.kind() => Ok(()),
            (Requirement::Kind(expected), value) => Err(ViolationKind::WrongKind {
                expected: *expected,
                actual: value.kind(),
            }),
            (Requirement::IntegerRange { minimum, maximum }, Value::Integer(actual)) => {
                if actual < minimum {
                    Err(ViolationKind::IntegerBelowMinimum {
                        minimum: *minimum,
                        actual: *actual,
                    })
                } else if actual > maximum {
                    Err(ViolationKind::IntegerAboveMaximum {
                        maximum: *maximum,
                        actual: *actual,
                    })
                } else {
                    Ok(())
                }
            }
            (Requirement::IntegerRange { .. }, value) => Err(ViolationKind::WrongKind {
                expected: ValueKind::Integer,
                actual: value.kind(),
            }),
            (Requirement::FloatRange { minimum, maximum }, Value::Float(actual)) => {
                if actual < minimum {
                    Err(ViolationKind::FloatBelowMinimum {
                        minimum: minimum.get(),
                        actual: actual.get(),
                    })
                } else if actual > maximum {
                    Err(ViolationKind::FloatAboveMaximum {
                        maximum: maximum.get(),
                        actual: actual.get(),
                    })
                } else {
                    Ok(())
                }
            }
            (Requirement::FloatRange { .. }, value) => Err(ViolationKind::WrongKind {
                expected: ValueKind::Float,
                actual: value.kind(),
            }),
            (Requirement::StringEnum(allowed), Value::String(actual)) => {
                if allowed.contains(actual) {
                    Ok(())
                } else {
                    Err(ViolationKind::StringNotAllowed {
                        actual: actual.clone(),
                    })
                }
            }
            (Requirement::StringEnum(_), value) => Err(ViolationKind::WrongKind {
                expected: ValueKind::String,
                actual: value.kind(),
            }),
        }
    }

    fn expected_kind(&self) -> Option<ValueKind> {
        match self.requirement {
            Requirement::Any => None,
            Requirement::Kind(kind) => Some(kind),
            Requirement::IntegerRange { .. } => Some(ValueKind::Integer),
            Requirement::FloatRange { .. } => Some(ValueKind::Float),
            Requirement::StringEnum(_) => Some(ValueKind::String),
        }
    }

    pub(crate) const fn null_policy(&self) -> NullPolicy {
        self.null_policy
    }

    pub(crate) fn shape(&self) -> ValidatorShape<'_> {
        match &self.requirement {
            Requirement::Any => ValidatorShape::Any,
            Requirement::Kind(kind) => ValidatorShape::Kind(*kind),
            Requirement::IntegerRange { minimum, maximum } => ValidatorShape::IntegerRange {
                minimum: *minimum,
                maximum: *maximum,
            },
            Requirement::FloatRange { minimum, maximum } => ValidatorShape::FloatRange {
                minimum: *minimum,
                maximum: *maximum,
            },
            Requirement::StringEnum(values) => ValidatorShape::StringEnum(values),
        }
    }
}

pub(crate) enum ValidatorShape<'a> {
    Any,
    Kind(ValueKind),
    IntegerRange {
        minimum: i64,
        maximum: i64,
    },
    FloatRange {
        minimum: FiniteF64,
        maximum: FiniteF64,
    },
    StringEnum(&'a BTreeSet<String>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueCast {
    StringToInteger,
    StringToFloat,
    StringToBool,
}

impl ValueCast {
    const fn target(self) -> ValueKind {
        match self {
            Self::StringToInteger => ValueKind::Integer,
            Self::StringToFloat => ValueKind::Float,
            Self::StringToBool => ValueKind::Bool,
        }
    }

    fn apply(self, value: Value) -> Result<Value, ViolationKind> {
        if value.kind() == self.target() || matches!(value, Value::Null) {
            return Ok(value);
        }
        let actual = value.kind();
        let Value::String(input) = value else {
            return Err(ViolationKind::CastFailed {
                target: self.target(),
                actual,
                reason: "only strings can be cast".to_owned(),
            });
        };
        let result = match self {
            Self::StringToInteger => input
                .parse::<i64>()
                .map(Value::Integer)
                .map_err(|error| error.to_string()),
            Self::StringToFloat => input
                .parse::<f64>()
                .map_err(|error| error.to_string())
                .and_then(|value| FiniteF64::new(value).map_err(|error| error.to_string()))
                .map(Value::Float),
            Self::StringToBool => input
                .parse::<bool>()
                .map(Value::Bool)
                .map_err(|error| error.to_string()),
        };
        result.map_err(|reason| ViolationKind::CastFailed {
            target: self.target(),
            actual,
            reason,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SchemaRule {
    selector: Selector,
    enforcement: Enforcement,
    validator: ValueValidator,
    cast: Option<ValueCast>,
    node_kind: Option<NodeKind>,
}

impl SchemaRule {
    pub fn new(
        selector: Selector,
        enforcement: Enforcement,
        validator: ValueValidator,
    ) -> Result<Self, SchemaRuleBuildError> {
        if selector.explicitly_targets_system() {
            return Err(SchemaRuleBuildError::SystemSelector { selector });
        }
        Ok(Self {
            selector,
            enforcement,
            validator,
            cast: None,
            node_kind: None,
        })
    }

    pub fn with_cast(mut self, cast: ValueCast) -> Result<Self, SchemaRuleBuildError> {
        let target = cast.target();
        if self.validator.expected_kind() != Some(target) {
            return Err(SchemaRuleBuildError::CastTargetMismatch {
                target,
                validator_kind: self.validator.expected_kind(),
            });
        }
        self.cast = Some(cast);
        Ok(self)
    }

    #[must_use]
    pub fn with_node_kind(mut self, node_kind: NodeKind) -> Self {
        self.node_kind = Some(node_kind);
        self
    }

    #[must_use]
    pub fn selector(&self) -> &Selector {
        &self.selector
    }

    pub(crate) const fn enforcement(&self) -> Enforcement {
        self.enforcement
    }

    pub(crate) fn validator(&self) -> &ValueValidator {
        &self.validator
    }

    pub(crate) const fn cast(&self) -> Option<ValueCast> {
        self.cast
    }

    pub(crate) const fn node_kind(&self) -> Option<NodeKind> {
        self.node_kind
    }

    pub fn validate(
        &self,
        topic: &TopicPath,
        value: &Value,
    ) -> Result<RuleOutcome, SchemaViolation> {
        if topic.is_system() || !self.selector.matches(topic) {
            return Ok(RuleOutcome::Valid);
        }
        let issue = match self.validator.validate(value) {
            Ok(()) => return Ok(RuleOutcome::Valid),
            Err(kind) => SchemaIssue {
                topic: topic.clone(),
                kind,
            },
        };
        match self.enforcement {
            Enforcement::Warn => Ok(RuleOutcome::Warning(issue)),
            Enforcement::Deny => Err(SchemaViolation(issue)),
        }
    }

    fn applies_to(&self, topic: &TopicPath) -> bool {
        !topic.is_system() && self.selector.matches(topic)
    }

    fn issue(&self, topic: &TopicPath, kind: ViolationKind) -> SchemaIssue {
        SchemaIssue {
            topic: topic.clone(),
            kind,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuleOutcome {
    Valid,
    Warning(SchemaIssue),
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SchemaRuleBuildError {
    #[error("schema selector `{selector}` explicitly targets the reserved system namespace")]
    SystemSelector { selector: Selector },
    #[error("cast target {target:?} does not match validator kind {validator_kind:?}")]
    CastTargetMismatch {
        target: ValueKind,
        validator_kind: Option<ValueKind>,
    },
}

#[derive(Clone, Debug)]
pub struct Schema {
    name: SchemaName,
    rules: Vec<SchemaRule>,
}

impl Schema {
    pub fn new(name: SchemaName, rules: Vec<SchemaRule>) -> Result<Self, SchemaBuildError> {
        if rules.is_empty() {
            return Err(SchemaBuildError::Empty);
        }
        for (index, left) in rules.iter().enumerate() {
            for right in &rules[index + 1..] {
                if left.cast.is_some()
                    && right.cast.is_some()
                    && left.selector.intersects(&right.selector)
                {
                    return Err(SchemaBuildError::AmbiguousCasts {
                        left: left.selector.clone(),
                        right: right.selector.clone(),
                    });
                }
            }
        }
        Ok(Self { name, rules })
    }

    #[must_use]
    pub fn name(&self) -> &SchemaName {
        &self.name
    }

    pub(crate) fn rules(&self) -> &[SchemaRule] {
        &self.rules
    }

    pub fn validate_value(
        &self,
        topic: &TopicPath,
        value: Value,
    ) -> Result<ValidatedValue, SchemaViolation> {
        let matching = self
            .rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.applies_to(topic))
            .collect::<Vec<_>>();
        let mut warnings = Vec::new();
        let mut skipped_validator = None;
        let mut value = value;

        if let Some((index, rule)) = matching
            .iter()
            .copied()
            .find(|(_, rule)| rule.cast.is_some())
        {
            match rule
                .cast
                .expect("matching cast was checked")
                .apply(value.clone())
            {
                Ok(cast) => value = cast,
                Err(kind) => {
                    let issue = rule.issue(topic, kind);
                    match rule.enforcement {
                        Enforcement::Deny => return Err(SchemaViolation(issue)),
                        Enforcement::Warn => {
                            warnings.push(issue);
                            skipped_validator = Some(index);
                        }
                    }
                }
            }
        }

        for (index, rule) in matching {
            if skipped_validator == Some(index) {
                continue;
            }
            if let Err(kind) = rule.validator.validate(&value) {
                let issue = rule.issue(topic, kind);
                match rule.enforcement {
                    Enforcement::Warn => warnings.push(issue),
                    Enforcement::Deny => return Err(SchemaViolation(issue)),
                }
            }
        }
        Ok(ValidatedValue { value, warnings })
    }

    pub(crate) fn inspect_existing(
        &self,
        topic: &TopicPath,
        node_kind: NodeKind,
        value: Option<&Value>,
    ) -> Vec<(Enforcement, SchemaIssue)> {
        self.rules
            .iter()
            .filter(|rule| rule.applies_to(topic))
            .flat_map(|rule| {
                let mut issues = Vec::new();
                if let Some(expected) = rule.node_kind.filter(|expected| *expected != node_kind) {
                    issues.push((
                        rule.enforcement,
                        rule.issue(
                            topic,
                            ViolationKind::WrongNodeKind {
                                expected,
                                actual: node_kind,
                            },
                        ),
                    ));
                }
                if let Some(value) = value
                    && let Err(kind) = rule.validator.validate(value)
                {
                    issues.push((rule.enforcement, rule.issue(topic, kind)));
                }
                issues
            })
            .collect()
    }

    fn validate_node_kind(
        &self,
        topic: &TopicPath,
        actual: NodeKind,
    ) -> Result<Vec<SchemaIssue>, SchemaViolation> {
        let mut warnings = Vec::new();
        for rule in self.rules.iter().filter(|rule| rule.applies_to(topic)) {
            let Some(expected) = rule.node_kind.filter(|expected| *expected != actual) else {
                continue;
            };
            let issue = rule.issue(topic, ViolationKind::WrongNodeKind { expected, actual });
            match rule.enforcement {
                Enforcement::Warn => warnings.push(issue),
                Enforcement::Deny => return Err(SchemaViolation(issue)),
            }
        }
        Ok(warnings)
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SchemaBuildError {
    #[error("a schema must contain at least one rule")]
    Empty,
    #[error("overlapping selectors `{left}` and `{right}` both define casts")]
    AmbiguousCasts { left: Selector, right: Selector },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedValue {
    value: Value,
    warnings: Vec<SchemaIssue>,
}

impl ValidatedValue {
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    #[must_use]
    pub fn warnings(&self) -> &[SchemaIssue] {
        &self.warnings
    }

    #[must_use]
    pub fn into_parts(self) -> (Value, Vec<SchemaIssue>) {
        (self.value, self.warnings)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SchemaRegistry {
    schemas: BTreeMap<SchemaName, Schema>,
}

impl SchemaRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.schemas.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
    }

    pub(crate) fn schemas(&self) -> impl Iterator<Item = &Schema> {
        self.schemas.values()
    }

    pub fn install(&mut self, schema: Schema) -> Result<Option<Schema>, SchemaRegistryError> {
        for existing in self.schemas.values() {
            if existing.name == schema.name {
                continue;
            }
            for candidate_rule in &schema.rules {
                for existing_rule in &existing.rules {
                    if candidate_rule.selector.intersects(&existing_rule.selector) {
                        return Err(SchemaRegistryError::Overlap {
                            candidate_schema: schema.name.clone(),
                            candidate_selector: candidate_rule.selector.clone(),
                            existing_schema: existing.name.clone(),
                            existing_selector: existing_rule.selector.clone(),
                        });
                    }
                }
            }
        }
        Ok(self.schemas.insert(schema.name.clone(), schema))
    }

    pub fn validate_value(
        &self,
        topic: &TopicPath,
        value: Value,
    ) -> Result<ValidatedValue, SchemaViolation> {
        let Some(schema) = self
            .schemas
            .values()
            .find(|schema| schema.rules.iter().any(|rule| rule.applies_to(topic)))
        else {
            return Ok(ValidatedValue {
                value,
                warnings: Vec::new(),
            });
        };
        schema.validate_value(topic, value)
    }

    pub fn validate_node_kind(
        &self,
        topic: &TopicPath,
        node_kind: NodeKind,
    ) -> Result<Vec<SchemaIssue>, SchemaViolation> {
        let Some(schema) = self
            .schemas
            .values()
            .find(|schema| schema.rules.iter().any(|rule| rule.applies_to(topic)))
        else {
            return Ok(Vec::new());
        };
        schema.validate_node_kind(topic, node_kind)
    }

    pub(crate) fn inspect_existing(
        &self,
        topic: &TopicPath,
        node_kind: NodeKind,
        value: Option<&Value>,
    ) -> Vec<(Enforcement, SchemaIssue)> {
        self.schemas
            .values()
            .find(|schema| schema.rules.iter().any(|rule| rule.applies_to(topic)))
            .map_or_else(Vec::new, |schema| {
                schema.inspect_existing(topic, node_kind, value)
            })
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum SchemaRegistryError {
    #[error(
        "schema `{candidate_schema}` selector `{candidate_selector}` overlaps schema `{existing_schema}` selector `{existing_selector}`"
    )]
    Overlap {
        candidate_schema: SchemaName,
        candidate_selector: Selector,
        existing_schema: SchemaName,
        existing_selector: Selector,
    },
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ValidatorBuildError {
    #[error("null acceptance must be configured with NullPolicy")]
    NullMustUsePolicy,
    #[error("integer range minimum {minimum} exceeds maximum {maximum}")]
    ReversedIntegerRange { minimum: i64, maximum: i64 },
    #[error("float range minimum {minimum} exceeds maximum {maximum}")]
    ReversedFloatRange { minimum: f64, maximum: f64 },
    #[error("a string enum must contain at least one value")]
    EmptyStringEnum,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaIssue {
    topic: TopicPath,
    kind: ViolationKind,
}

impl SchemaIssue {
    #[must_use]
    pub fn topic(&self) -> &TopicPath {
        &self.topic
    }

    #[must_use]
    pub const fn kind(&self) -> &ViolationKind {
        &self.kind
    }
}

impl fmt::Display for SchemaIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "schema violation at {}: {}",
            self.topic, self.kind
        )
    }
}

#[derive(Clone, Debug, Error, PartialEq)]
#[error("{0}")]
pub struct SchemaViolation(SchemaIssue);

impl SchemaViolation {
    #[must_use]
    pub fn topic(&self) -> &TopicPath {
        self.0.topic()
    }

    #[must_use]
    pub const fn kind(&self) -> &ViolationKind {
        self.0.kind()
    }
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum ViolationKind {
    #[error("null is not allowed")]
    NullNotAllowed,
    #[error("expected {expected:?}, received {actual:?}")]
    WrongKind {
        expected: ValueKind,
        actual: ValueKind,
    },
    #[error("integer {actual} is below minimum {minimum}")]
    IntegerBelowMinimum { minimum: i64, actual: i64 },
    #[error("integer {actual} is above maximum {maximum}")]
    IntegerAboveMaximum { maximum: i64, actual: i64 },
    #[error("float {actual} is below minimum {minimum}")]
    FloatBelowMinimum { minimum: f64, actual: f64 },
    #[error("float {actual} is above maximum {maximum}")]
    FloatAboveMaximum { maximum: f64, actual: f64 },
    #[error("string `{actual}` is not in the allowed set")]
    StringNotAllowed { actual: String },
    #[error("expected node kind {expected:?}, received {actual:?}")]
    WrongNodeKind {
        expected: NodeKind,
        actual: NodeKind,
    },
    #[error("could not cast {actual:?} to {target:?}: {reason}")]
    CastFailed {
        target: ValueKind,
        actual: ValueKind,
        reason: String,
    },
}
