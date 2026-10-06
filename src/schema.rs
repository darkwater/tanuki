use std::{collections::BTreeSet, fmt};

use thiserror::Error;

use crate::domain::{FiniteF64, Selector, TopicPath, Value, ValueKind};

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
}

#[derive(Clone, Debug)]
pub struct SchemaRule {
    selector: Selector,
    enforcement: Enforcement,
    validator: ValueValidator,
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
        })
    }

    #[must_use]
    pub fn selector(&self) -> &Selector {
        &self.selector
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
}
