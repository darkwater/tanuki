use tanuki_protocol::{FiniteF64, JsonValue, Value};

#[test]
fn truncated_messagepack_array_returns_a_decode_error() {
    // array32 with a maximum length, but no elements. No large payload exists.
    assert!(rmp_serde::from_slice::<JsonValue>(&[0xdd, 0xff, 0xff, 0xff, 0xff]).is_err());
}

#[test]
fn finite_float_bits_survive_json_and_messagepack_round_trips() {
    // Fixed seed covers small/large exponents and mantissas without a random dependency.
    let mut bits = 0x1234_5678_9abc_def0_u64;
    for value in [0.0, -0.0, f64::MIN_POSITIVE, f64::MAX, f64::from_bits(1)]
        .into_iter()
        .chain((0..10_000).map(|_| {
            bits ^= bits << 13;
            bits ^= bits >> 7;
            bits ^= bits << 17;
            f64::from_bits(bits)
        }))
        .filter(|value| value.is_finite())
    {
        let original = JsonValue::new(Value::Float(FiniteF64::new(value).unwrap()));
        let json = serde_json::to_string(&original).unwrap();
        let binary = rmp_serde::to_vec_named(&original).unwrap();
        for decoded in [
            serde_json::from_str::<JsonValue>(&json).unwrap(),
            rmp_serde::from_slice::<JsonValue>(&binary).unwrap(),
        ] {
            let Value::Float(decoded) = decoded.into_inner() else {
                panic!("float became a different runtime kind: {json}")
            };
            assert_eq!(decoded.get().to_bits(), value.to_bits(), "{json}");
        }
    }
}
