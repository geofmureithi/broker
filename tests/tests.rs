extern crate broker;
use serde_json::json;
use base64::encode;

#[tokio::test]
async fn test1() {

    let user1 = json!({"username": "rust22", "password": "rust", "collection_id":"3ca76743-8d99-4d3f-b85c-633ea456f90c", "tenant_id": "e69d88c2-135e-4280-9cd8-d4a5edd8642a"});
    let user2 = json!({"username": "rust23", "password": "rust", "collection_id":"3ca76743-8d99-4d3f-b85c-633ea456f90d", "tenant_id": "e69d88c2-135e-4280-9cd8-d4a5edd8642a"});
    let user1_login = json!({"username": "rust22", "password": "rust"});
    let event1 = json!({"event": "test", "tenant_id": "e69d88c2-135e-4280-9cd8-d4a5edd8642a", "collection_id": "3ca76743-8d99-4d3f-b85c-633ea456f90c", "timestamp": 1578667309, "data": "{}"});
    let now = broker::get_ntp_time();
    let x = now + 1000;
    let event2 = json!({"event": "user", "tenant_id": "e69d88c2-135e-4280-9cd8-d4a5edd8642a", "collection_id": "3ca76743-8d99-4d3f-b85c-633ea456f90d", "timestamp": x, "data": "{}"});

    let client = reqwest::Client::new();

    let basic_token = encode("rust22:rust");
    let basic = format!("Basic {}", basic_token);

    // create user 1 - want success
    let res = client.post("http://localhost:8080/users")
        .json(&user1)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);

    // create user 2 - want success
    let res = client.post("http://localhost:8080/users")
        .json(&user2)
        .send().await.unwrap()
        .status();
    assert_eq!(res, 200);

    // try to create user 2 again - want failure
    let res = client.post("http://localhost:8080/users")
        .json(&user1)
        .send().await.unwrap()
        .status();
    assert_eq!(res, 400);

    // login for user 1 - want success
    let res = client.post("http://localhost:8080/login")
        .json(&user1_login)
        .send().await.unwrap()
        .text().await.unwrap();
    
    let token: broker::Token = serde_json::from_str(&res).unwrap();
    let bearer = format!("Bearer {}", token.jwt);

    // try posting event without auth - want failure
    let res = client.post("http://localhost:8080/insert")
        .json(&event1)
        .send().await.unwrap()
        .status();
    assert_eq!(res, 400);

    // try posting event with bad auth - want failure
    let res = client.post("http://localhost:8080/insert")
        .header("Authorization", "foo")
        .json(&event1)
        .send().await.unwrap()
        .status();
    assert_eq!(res, 401);

    // try posting event with bad auth - want failure
    let res = client.post("http://localhost:8080/insert")
        .header("Authorization", "Bearer 1234")
        .json(&event1)
        .send().await.unwrap()
        .status();
    assert_eq!(res, 401);

    // post event with JWT - want success
    let res = client.post("http://localhost:8080/insert")
        .header("Authorization", &bearer)
        .json(&event1)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    let event : broker::Record = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(event.event.published, false);

    // post event with JWT - want success
    let res = client.post("http://localhost:8080/insert")
        .header("Authorization", &bearer)
        .json(&event2)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    let event2 : broker::Record = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(event2.event.published, false);

    // post event with HTTP Basic - want success
    let res = client.post("http://localhost:8080/insert")
        .header("Authorization", &basic)
        .json(&event1)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    let event : broker::Record = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(event.event.published, false);

    // try getting collection without auth - want failure
    let res = client.get("http://localhost:8080/collections/123")
        .send().await.unwrap()
        .status();
    assert_eq!(res, 400);

    // pause for a second to process job
    let half_second = std::time::Duration::from_millis(500);
    std::thread::sleep(half_second);

    // get collection with JWT - want success
    let res = client.get("http://localhost:8080/collections/3ca76743-8d99-4d3f-b85c-633ea456f90c")
        .header("Authorization", &bearer)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    let events : broker::Collection = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(events.events[0].published, true);

    // get collection with HTTP Basic - want success
    let res = client.get("http://localhost:8080/collections/3ca76743-8d99-4d3f-b85c-633ea456f90c")
        .header("Authorization", &basic)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    let events : broker::Collection = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(events.events[0].published, true);

    // try getting user without auth - want failure
    let res = client.get("http://localhost:8080/user_events")
        .send().await.unwrap()
        .status();
    assert_eq!(res, 400);

    // get user collection - want success
    let res = client.get("http://localhost:8080/user_events")
        .header("Authorization", &bearer)
        .send().await.unwrap()
        .status();
    assert_eq!(res, 200);

    // try cancelling without auth - want failure
    let res = client.get("http://localhost:8080/cancel/123")
        .send().await.unwrap()
        .status();
    assert_eq!(res, 400);

    // cancel with JWT - want success
    let url = format!("http://localhost:8080/cancel/{}", event2.event.id);
    let res = client.get(&url)
        .header("Authorization", &bearer)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    let event : broker::Record = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(event.event.cancelled, true);

    // cancel with HTTP Basic - want success
    let url = format!("http://localhost:8080/cancel/{}", event2.event.id);
    let res = client.get(&url)
        .header("Authorization", &basic)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    let event : broker::Record = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(event.event.cancelled, true);

    // get collection with JWT - want success
    let res = client.get("http://localhost:8080/collections/3ca76743-8d99-4d3f-b85c-633ea456f90d")
        .header("Authorization", &bearer)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    let events : broker::Collection = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(events.events[0].published, false);

    // get collection with HTTP Basic - want success
    let res = client.get("http://localhost:8080/collections/3ca76743-8d99-4d3f-b85c-633ea456f90d")
        .header("Authorization", &basic)
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    let events : broker::Collection = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(events.events[0].published, false);
}
