use tanuki::{
    domain::{FiniteF64, Selector, TopicPath, Value, ValueKind},
    schema::{
        Enforcement, NullPolicy, RuleOutcome, Schema, SchemaBuildError, SchemaName, SchemaRegistry,
        SchemaRegistryError, SchemaRule, SchemaRuleBuildError, ValidatorBuildError, ValueCast,
        ValueValidator, ViolationKind,
    },
};

fn topic(value: &str) -> TopicPath {
    TopicPath::parse(value).unwrap()
}

#[test]
fn warning_and_deny_rules_report_the_same_structured_range_violation() {
    let validator = ValueValidator::integer_range(0, 100, NullPolicy::Deny).unwrap();
    let warning = SchemaRule::new(
        Selector::parse("/battery/*").unwrap(),
        Enforcement::Warn,
        validator.clone(),
    )
    .unwrap();
    let denying = SchemaRule::new(
        Selector::parse("/battery/*").unwrap(),
        Enforcement::Deny,
        validator,
    )
    .unwrap();

    let RuleOutcome::Warning(warning) = warning
        .validate(&topic("/battery/phone"), &Value::Integer(101))
        .unwrap()
    else {
        panic!("warning enforcement must accept with a diagnostic");
    };
    let denying = denying
        .validate(&topic("/battery/phone"), &Value::Integer(101))
        .unwrap_err();
    assert_eq!(warning.kind(), denying.kind());
    assert_eq!(
        warning.kind(),
        &ViolationKind::IntegerAboveMaximum {
            maximum: 100,
            actual: 101,
        }
    );
}

#[test]
fn null_policy_is_independent_of_the_expected_non_null_kind() {
    let nullable = SchemaRule::new(
        Selector::parse("/optional").unwrap(),
        Enforcement::Deny,
        ValueValidator::kind(ValueKind::String, NullPolicy::Allow).unwrap(),
    )
    .unwrap();
    assert!(nullable.validate(&topic("/optional"), &Value::Null).is_ok());

    let required = SchemaRule::new(
        Selector::parse("/required").unwrap(),
        Enforcement::Deny,
        ValueValidator::kind(ValueKind::String, NullPolicy::Deny).unwrap(),
    )
    .unwrap();
    assert_eq!(
        required
            .validate(&topic("/required"), &Value::Null)
            .unwrap_err()
            .kind(),
        &ViolationKind::NullNotAllowed
    );
}

#[test]
fn primitive_kind_float_range_and_string_enum_are_checked_without_casting() {
    let float = ValueValidator::float_range(
        FiniteF64::new(-1.5).unwrap(),
        FiniteF64::new(1.5).unwrap(),
        NullPolicy::Deny,
    )
    .unwrap();
    assert!(
        float
            .validate(&Value::Float(FiniteF64::new(1.5).unwrap()))
            .is_ok()
    );
    assert_eq!(
        float.validate(&Value::Integer(1)).unwrap_err(),
        ViolationKind::WrongKind {
            expected: ValueKind::Float,
            actual: ValueKind::Integer,
        }
    );

    let strings = ValueValidator::string_enum(
        ["charging".to_owned(), "discharging".to_owned()],
        NullPolicy::Deny,
    )
    .unwrap();
    assert!(
        strings
            .validate(&Value::String("charging".to_owned()))
            .is_ok()
    );
    assert!(matches!(
        strings.validate(&Value::String("unknown".to_owned())),
        Err(ViolationKind::StringNotAllowed { .. })
    ));
}

#[test]
fn invalid_validator_definitions_are_rejected_at_construction() {
    assert_eq!(
        ValueValidator::integer_range(10, 0, NullPolicy::Deny).unwrap_err(),
        ValidatorBuildError::ReversedIntegerRange {
            minimum: 10,
            maximum: 0,
        }
    );
    assert_eq!(
        ValueValidator::kind(ValueKind::Null, NullPolicy::Allow).unwrap_err(),
        ValidatorBuildError::NullMustUsePolicy
    );
    assert_eq!(
        ValueValidator::string_enum([], NullPolicy::Deny).unwrap_err(),
        ValidatorBuildError::EmptyStringEnum
    );
}

#[test]
fn schema_rules_ignore_system_topics_and_identify_explicit_system_branches() {
    let broad = SchemaRule::new(
        Selector::parse("/**").unwrap(),
        Enforcement::Deny,
        ValueValidator::kind(ValueKind::Integer, NullPolicy::Deny).unwrap(),
    )
    .unwrap();
    assert!(
        broad
            .validate(
                &topic("/$connections/client"),
                &Value::String("system".to_owned())
            )
            .is_ok()
    );
    assert!(!broad.selector().explicitly_targets_system());

    assert!(
        Selector::parse("/$schemas/battery")
            .unwrap()
            .explicitly_targets_system()
    );
    assert!(
        Selector::parse("/{$schemas,battery}/**")
            .unwrap()
            .explicitly_targets_system()
    );
    assert!(
        !Selector::parse("/*/**")
            .unwrap()
            .explicitly_targets_system()
    );

    assert!(matches!(
        SchemaRule::new(
            Selector::parse("/$schemas/battery").unwrap(),
            Enforcement::Deny,
            ValueValidator::any(NullPolicy::Allow),
        ),
        Err(SchemaRuleBuildError::SystemSelector { .. })
    ));
}

fn named_schema(name: &str, rules: Vec<SchemaRule>) -> Schema {
    Schema::new(SchemaName::parse(name).unwrap(), rules).unwrap()
}

#[test]
fn explicit_string_cast_is_applied_once_then_all_matching_rules_revalidate() {
    let cast = SchemaRule::new(
        Selector::parse("/battery/*").unwrap(),
        Enforcement::Deny,
        ValueValidator::kind(ValueKind::Integer, NullPolicy::Deny).unwrap(),
    )
    .unwrap()
    .with_cast(ValueCast::StringToInteger)
    .unwrap();
    let range = SchemaRule::new(
        Selector::parse("/battery/{phone,laptop}").unwrap(),
        Enforcement::Deny,
        ValueValidator::integer_range(0, 100, NullPolicy::Deny).unwrap(),
    )
    .unwrap();
    let schema = named_schema("battery", vec![cast, range]);

    let validated = schema
        .validate_value(&topic("/battery/phone"), Value::String("42".to_owned()))
        .unwrap();
    assert_eq!(validated.value(), &Value::Integer(42));
    assert!(validated.warnings().is_empty());

    let error = schema
        .validate_value(&topic("/battery/phone"), Value::String("101".to_owned()))
        .unwrap_err();
    assert!(matches!(
        error.kind(),
        ViolationKind::IntegerAboveMaximum {
            maximum: 100,
            actual: 101
        }
    ));
}

#[test]
fn failed_cast_obeys_warning_or_deny_enforcement() {
    let make = |enforcement| {
        named_schema(
            "cast",
            vec![
                SchemaRule::new(
                    Selector::parse("/value").unwrap(),
                    enforcement,
                    ValueValidator::kind(ValueKind::Bool, NullPolicy::Deny).unwrap(),
                )
                .unwrap()
                .with_cast(ValueCast::StringToBool)
                .unwrap(),
            ],
        )
    };

    let warned = make(Enforcement::Warn)
        .validate_value(&topic("/value"), Value::String("yes".to_owned()))
        .unwrap();
    assert_eq!(warned.value(), &Value::String("yes".to_owned()));
    assert!(matches!(
        warned.warnings()[0].kind(),
        ViolationKind::CastFailed {
            target: ValueKind::Bool,
            ..
        }
    ));

    let denied = make(Enforcement::Deny)
        .validate_value(&topic("/value"), Value::String("yes".to_owned()))
        .unwrap_err();
    assert!(matches!(
        denied.kind(),
        ViolationKind::CastFailed {
            target: ValueKind::Bool,
            ..
        }
    ));
}

#[test]
fn schema_allows_overlapping_validators_but_rejects_ambiguous_casts() {
    let validator = |selector: &str| {
        SchemaRule::new(
            Selector::parse(selector).unwrap(),
            Enforcement::Deny,
            ValueValidator::kind(ValueKind::Integer, NullPolicy::Deny).unwrap(),
        )
        .unwrap()
    };
    assert!(
        Schema::new(
            SchemaName::parse("valid overlap").unwrap(),
            vec![validator("/battery/*"), validator("/battery/phone")],
        )
        .is_ok()
    );

    let result = Schema::new(
        SchemaName::parse("ambiguous casts").unwrap(),
        vec![
            validator("/battery/*")
                .with_cast(ValueCast::StringToInteger)
                .unwrap(),
            validator("/battery/phone")
                .with_cast(ValueCast::StringToInteger)
                .unwrap(),
        ],
    );
    assert!(matches!(
        result,
        Err(SchemaBuildError::AmbiguousCasts { .. })
    ));
}

#[test]
fn registry_rejects_cross_schema_overlap_without_replacing_the_old_schema() {
    let rule = |selector: &str| {
        SchemaRule::new(
            Selector::parse(selector).unwrap(),
            Enforcement::Deny,
            ValueValidator::any(NullPolicy::Allow),
        )
        .unwrap()
    };
    let mut registry = SchemaRegistry::new();
    registry
        .install(named_schema("battery", vec![rule("/battery/*")]))
        .unwrap();
    let result = registry.install(named_schema("phone only", vec![rule("/battery/phone")]));
    assert!(matches!(result, Err(SchemaRegistryError::Overlap { .. })));
    assert_eq!(registry.len(), 1);

    registry
        .install(named_schema("battery", vec![rule("/power/*")]))
        .unwrap();
    assert_eq!(registry.len(), 1);
    assert!(
        registry
            .validate_value(&topic("/battery/phone"), Value::Integer(1))
            .is_ok()
    );
}
