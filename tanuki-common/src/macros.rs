#[macro_export]
macro_rules! property {
    attr($namespace:ty, $kind:ident, key = $key:expr) (
        $( #[ $meta:meta ] )*
        pub $itemty:tt $ident:ident $rest:tt $($semicolon:tt)?
    ) => {
        #[derive(Debug, Clone, PartialEq, $crate::_serde::Serialize, $crate::_serde::Deserialize)]
        $( #[ $meta ] )*
        pub $itemty $ident $rest $($semicolon)?

        impl $crate::Property for $ident {
            const KEY: &str = $key;
            const KIND: $crate::property::PropertyKind = $crate::property::PropertyKind::$kind;
        }

        impl $namespace for $ident {}
    };
}

#[cfg(test)]
mod tests {

    #[test]
    fn properties() {
        use crate::Property;

        #[expect(unused)]
        trait TestProperty: Property {}

        #[property(TestProperty, State, key = "foo")]
        pub struct Foo(pub u32);

        assert_eq!(Foo::KEY, "foo");
        assert_eq!(serde_json::to_value(Foo(42)).unwrap(), serde_json::json!(42));

        #[property(TestProperty, State, key = "bar")]
        pub struct Bar {
            pub name: String,
            pub value: f64,
        }

        assert_eq!(Bar::KEY, "bar");
        assert_eq!(
            serde_json::to_value(Bar {
                name: "example".to_owned(),
                value: 21.4,
            })
            .unwrap(),
            serde_json::json!({
                "name": "example",
                "value": 21.4,
            }),
        );
    }
}
