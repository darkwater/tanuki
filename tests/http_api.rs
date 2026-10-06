use std::sync::{Arc, Mutex};

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use jiff::Timestamp as JiffTimestamp;
use serde_json::{Value as JsonValue, json};
use tanuki::{
    core::Core,
    domain::{
        ClientName, ExpiryUpdate, Node, Timestamp, TopicPath, Value, WriteBatch, WriteContext,
        WriteOperation,
    },
    transport::{Clock, router_with_clock},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

fn test_app() -> (axum::Router, Arc<Mutex<Core>>) {
    let core = Arc::new(Mutex::new(Core::new()));
    let clock: Clock =
        Arc::new(|| Timestamp::new(JiffTimestamp::from_second(1_700_000_000).unwrap()));
    (router_with_clock(core.clone(), clock), core)
}

fn json_request(method: &str, uri: &str, body: JsonValue) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn response_json(response: axum::response::Response) -> JsonValue {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn schema_installation_casts_valid_http_writes_and_denies_invalid_ones() {
    let (app, core) = test_app();
    let unattributed = app
        .clone()
        .oneshot(json_request(
            "PUT",
            "/v1/schemas/battery",
            json!({"rules": []}),
        ))
        .await
        .unwrap();
    assert_eq!(unattributed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(unattributed).await["error"]["code"],
        "missing_client"
    );

    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            "/v1/schemas/battery?client=administrator",
            json!({
                "rules": [{
                    "selector": "/battery/*",
                    "enforcement": "deny",
                    "validator": {"type": "integer_range", "minimum": 0, "maximum": 100},
                    "cast": "string_to_integer",
                    "expected_update_interval": "PT5M"
                }]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/state/battery/phone?client=test",
            json!({"value": "42"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = core
        .lock()
        .unwrap()
        .read(&tanuki::domain::Selection::new(vec![
            tanuki::domain::Selector::parse("/battery/phone").unwrap(),
        ]));
    let Some(Node::State(state)) = snapshot.nodes().values().next() else {
        panic!("battery state must exist");
    };
    assert_eq!(state.current().value(), &Value::Integer(42));

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/state/battery/phone?client=test",
            json!({"value": 101}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "schema_violation"
    );
}

#[tokio::test]
async fn attributed_link_install_projects_reads_and_routes_alias_writes() {
    let (app, core) = test_app();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/state/devices/lamp/power?client=device",
            json!({"value": false}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let unattributed = app
        .clone()
        .oneshot(json_request(
            "PUT",
            "/v1/links/living-room",
            json!({"mount": "/rooms/living-room", "target": "/devices/lamp"}),
        ))
        .await
        .unwrap();
    assert_eq!(unattributed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(unattributed).await["error"]["code"],
        "missing_client"
    );

    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            "/v1/links/living-room?client=administrator",
            json!({"mount": "/rooms/living-room", "target": "/devices/lamp"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let installed = response_json(response).await;
    assert_eq!(installed["data"]["enabled"], true);

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/state/rooms/living-room/power?client=wall-panel",
            json!({"value": true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let snapshot = core
        .lock()
        .unwrap()
        .read(&tanuki::domain::Selection::new(vec![
            tanuki::domain::Selector::parse("/**").unwrap(),
        ]));
    for path in ["/devices/lamp/power", "/rooms/living-room/power"] {
        let Some(Node::State(state)) = snapshot.nodes().get(&TopicPath::parse(path).unwrap())
        else {
            panic!("{path} must be visible as state");
        };
        assert_eq!(state.current().value(), &Value::Bool(true));
    }

    let response = app
        .oneshot(json_request(
            "DELETE",
            "/v1/links/living-room?client=administrator",
            json!(null),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["data"]["removed"], true);
    let snapshot = core
        .lock()
        .unwrap()
        .read(&tanuki::domain::Selection::new(vec![
            tanuki::domain::Selector::parse("/**").unwrap(),
        ]));
    assert!(
        snapshot
            .nodes()
            .contains_key(&TopicPath::parse("/devices/lamp/power").unwrap())
    );
    assert!(
        !snapshot
            .nodes()
            .contains_key(&TopicPath::parse("/rooms/living-room/power").unwrap())
    );
}

#[tokio::test]
async fn schema_install_reports_existing_violations_and_force_removes_invalid_state() {
    let (app, core) = test_app();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/state/battery/phone?client=test",
            json!({"value": "unknown"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let declaration = json!({
        "rules": [{
            "selector": "/battery/*",
            "enforcement": "deny",
            "validator": {"type": "integer_range", "minimum": 0, "maximum": 100}
        }]
    });

    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            "/v1/schemas/battery?client=administrator",
            declaration.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error = response_json(response).await;
    assert_eq!(error["error"]["code"], "existing_schema_violations");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("/battery/phone")
    );

    let mut forced = declaration;
    forced["force"] = json!(true);
    let response = app
        .oneshot(json_request(
            "PUT",
            "/v1/schemas/battery?client=administrator",
            forced,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        core.lock()
            .unwrap()
            .read(&tanuki::domain::Selection::new(vec![
                tanuki::domain::Selector::parse("/battery/phone").unwrap(),
            ]))
            .nodes()
            .is_empty()
    );
}

#[tokio::test]
async fn malformed_json_and_missing_attribution_use_the_common_error_shape() {
    let (app, _) = test_app();
    let malformed = Request::builder()
        .method("POST")
        .uri("/v1/write")
        .header("content-type", "application/json")
        .header("tanuki-client", "phone")
        .body(Body::from("{"))
        .unwrap();
    let response = app.clone().oneshot(malformed).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "invalid_json"
    );

    let malformed_schema = Request::builder()
        .method("PUT")
        .uri("/v1/schemas/broken?client=administrator")
        .header("content-type", "application/json")
        .body(Body::from("{"))
        .unwrap();
    let response = app.clone().oneshot(malformed_schema).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "invalid_json"
    );

    let response = app
        .oneshot(json_request("POST", "/v1/write", json!({"operations": []})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "missing_client"
    );
}

#[tokio::test]
async fn stateless_attribution_accepts_query_and_rejects_conflicts() {
    let (app, core) = test_app();
    let request = json_request(
        "POST",
        "/v1/state/battery/phone?client=phone%20task",
        json!({"value": 72}),
    );
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let snapshot = core
        .lock()
        .unwrap()
        .read(&tanuki::domain::Selection::new(vec![
            tanuki::domain::Selector::parse("/battery/phone").unwrap(),
        ]));
    let Node::State(node) = snapshot.nodes().values().next().unwrap() else {
        panic!("battery topic must be state");
    };
    assert_eq!(node.current().last_write().client().as_str(), "phone task");

    let mut request = json_request(
        "POST",
        "/v1/state/battery/phone?client=another",
        json!({"value": 73}),
    );
    request
        .headers_mut()
        .insert("tanuki-client", "phone task".parse().unwrap());
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "conflicting_client"
    );
}

#[tokio::test]
async fn routing_failures_also_use_the_common_error_shape() {
    let (app, _) = test_app();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response_json(response).await["error"]["code"], "not_found");

    let response = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/v1/snapshot")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "method_not_allowed"
    );
}

#[tokio::test]
async fn domain_failures_use_the_same_error_envelope() {
    let (app, _) = test_app();
    let mut request = json_request(
        "POST",
        "/v1/write",
        json!({
            "operations": [{
                "op": "publish_state",
                "topic": "/$connections/fake",
                "value": true,
                "expiry": {"mode": "clear"}
            }]
        }),
    );
    request
        .headers_mut()
        .insert("tanuki-client", "phone".parse().unwrap());
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "system_topic"
    );
}

#[tokio::test]
async fn batch_endpoint_composes_unclaimed_input_definition_and_submission() {
    let (app, core) = test_app();
    let mut request = json_request(
        "POST",
        "/v1/write",
        json!({
            "operations": [
                {"op": "define_input", "topic": "/heating/desired", "kind": "desired"},
                {"op": "submit_desired", "topic": "/heating/desired", "value": 21, "expiry": {"mode": "clear"}}
            ]
        }),
    );
    request
        .headers_mut()
        .insert("tanuki-client", "phone automation".parse().unwrap());
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(core.lock().unwrap().managed_session_count(), 0);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/snapshot?select=/heating/desired")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response_json(response).await;
    assert_eq!(body["data"]["nodes"]["/heating/desired"]["kind"], "desired");
    assert_eq!(
        body["data"]["nodes"]["/heating/desired"]["current"]["value"],
        21
    );
    assert!(body["data"]["nodes"]["/heating/desired"]["claim"].is_null());
}

#[tokio::test]
async fn snapshot_distinguishes_missing_desired_payload_from_submitted_null() {
    let (app, _) = test_app();
    let mut request = json_request(
        "POST",
        "/v1/write",
        json!({
            "operations": [
                {"op": "define_input", "topic": "/request/missing", "kind": "desired"},
                {"op": "define_input", "topic": "/request/null", "kind": "desired"},
                {"op": "submit_desired", "topic": "/request/null", "value": null, "expiry": {"mode": "clear"}}
            ]
        }),
    );
    request
        .headers_mut()
        .insert("tanuki-client", "test".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::OK
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/snapshot?select=/request/*")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response_json(response).await;
    assert!(body["data"]["nodes"]["/request/missing"]["current"].is_null());
    assert!(body["data"]["nodes"]["/request/null"]["current"].is_object());
    assert!(body["data"]["nodes"]["/request/null"]["current"]["value"].is_null());
}

#[tokio::test]
async fn stateless_battery_publish_and_anonymous_read_cross_the_real_router() {
    let (app, core) = test_app();
    let mut request = json_request(
        "POST",
        "/v1/state/battery/phone",
        json!({"value": 72, "expiry": {"mode": "clear"}}),
    );
    request
        .headers_mut()
        .insert("tanuki-client", "phone tasker".parse().unwrap());
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(core.lock().unwrap().managed_session_count(), 0);

    let request = Request::builder()
        .uri("/v1/snapshot?select=/battery/*")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(
        body["data"]["nodes"]["/battery/phone"]["current"]["value"],
        72
    );
    assert_eq!(
        body["data"]["nodes"]["/battery/phone"]["current"]["last_write"]["client"],
        "phone tasker"
    );
    assert_eq!(core.lock().unwrap().managed_session_count(), 0);
}

#[tokio::test]
async fn same_name_http_write_does_not_displace_a_managed_session() {
    let (app, core) = test_app();
    let handle = core
        .lock()
        .unwrap()
        .open_session(
            ClientName::parse("laptop agent").unwrap(),
            Timestamp::new(JiffTimestamp::from_second(1).unwrap()),
        )
        .unwrap()
        .handle()
        .clone();
    let mut request = json_request(
        "POST",
        "/v1/state/battery/phone",
        json!({"value": 72, "expiry": {"mode": "clear"}}),
    );
    request
        .headers_mut()
        .insert("tanuki-client", "laptop agent".parse().unwrap());
    assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::OK);

    let managed = WriteContext::managed(handle);
    core.lock()
        .unwrap()
        .apply(
            &managed,
            WriteBatch::new(vec![WriteOperation::PublishState {
                topic: TopicPath::parse("/battery/laptop").unwrap(),
                value: Value::Integer(80),
                expiry: ExpiryUpdate::Clear,
            }])
            .unwrap(),
            Timestamp::new(JiffTimestamp::from_second(2).unwrap()),
        )
        .unwrap();
    assert_eq!(core.lock().unwrap().managed_session_count(), 1);
}

#[tokio::test]
async fn omitted_expiry_preserves_an_existing_absolute_deadline() {
    let (app, core) = test_app();
    let mut first = json_request(
        "POST",
        "/v1/state/battery/phone",
        json!({"value": 70, "expiry": {"mode": "set", "duration": "PT1H"}}),
    );
    first
        .headers_mut()
        .insert("tanuki-client", "phone".parse().unwrap());
    assert_eq!(
        app.clone().oneshot(first).await.unwrap().status(),
        StatusCode::OK
    );
    let original_deadline = core
        .lock()
        .unwrap()
        .read(&tanuki::domain::Selection::new(vec![
            tanuki::domain::Selector::parse("/battery/phone").unwrap(),
        ]))
        .nodes()
        .get(&TopicPath::parse("/battery/phone").unwrap())
        .unwrap()
        .retained_value()
        .unwrap()
        .expires_at();

    let mut refresh = json_request("POST", "/v1/state/battery/phone", json!({"value": 71}));
    refresh
        .headers_mut()
        .insert("tanuki-client", "phone".parse().unwrap());
    assert_eq!(app.oneshot(refresh).await.unwrap().status(), StatusCode::OK);
    let snapshot = core
        .lock()
        .unwrap()
        .read(&tanuki::domain::Selection::new(vec![
            tanuki::domain::Selector::parse("/battery/phone").unwrap(),
        ]));
    let retained = snapshot
        .nodes()
        .get(&TopicPath::parse("/battery/phone").unwrap())
        .unwrap()
        .retained_value()
        .unwrap();
    assert_eq!(retained.value(), &Value::Integer(71));
    assert_eq!(retained.expires_at(), original_deadline);
}

#[tokio::test]
async fn tagged_values_and_escaped_maps_survive_json_storage() {
    let (app, _) = test_app();
    let value = json!({
        "bytes": {"$bytes": "AAEC/w=="},
        "at": {"$timestamp": "2023-11-14T22:13:20Z"},
        "elapsed": {"$duration": "PT1H2M3S"},
        "literal": {"$map": {"$bytes": "not a tag"}}
    });
    let mut request = json_request(
        "POST",
        "/v1/state/codec/sample",
        json!({"value": value, "expiry": {"mode": "clear"}}),
    );
    request
        .headers_mut()
        .insert("tanuki-client", "codec test".parse().unwrap());
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/snapshot?select=/codec/sample")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response_json(response).await;
    assert_eq!(
        body["data"]["nodes"]["/codec/sample"]["current"]["value"],
        value
    );
}

#[tokio::test]
async fn battery_publish_and_read_work_over_a_real_tcp_listener() {
    let core = Arc::new(Mutex::new(Core::new()));
    let clock: Clock =
        Arc::new(|| Timestamp::new(JiffTimestamp::from_second(1_700_000_000).unwrap()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(tanuki::server::serve_with_core(
        listener,
        core,
        clock,
        async move {
            let _ = shutdown_rx.await;
        },
    ));

    let body = r#"{"value":64,"expiry":{"mode":"clear"}}"#;
    let response = raw_http(
        address,
        &format!(
            "POST /v1/state/battery/phone HTTP/1.1\r\nHost: localhost\r\nTanuki-Client: phone tasker\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");

    let response = raw_http(
        address,
        "GET /v1/snapshot?select=/battery/* HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    let (_, body) = response.split_once("\r\n\r\n").unwrap();
    let body: JsonValue = serde_json::from_str(body).unwrap();
    assert_eq!(
        body["data"]["nodes"]["/battery/phone"]["current"]["value"],
        64
    );

    shutdown_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

async fn raw_http(address: std::net::SocketAddr, request: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8(response).unwrap()
}
