//! Passkey accounts through the router: sign-up on one device, discoverable
//! sign-in on another (a synced passkey), and the guard rails around them.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use axum::http::StatusCode;
use common::authenticator::{Passkey, READER};
use common::{Harness, TestDevice, harness, register};
use serde_json::{Value, json};

async fn call(
    h: &Harness,
    dev: &TestDevice,
    token: &str,
    path: &str,
    body: &Value,
) -> common::Response {
    common::post_json(h, dev, token, path, body).await
}

const BEGIN: &str = "/kernel/v1/auth/passkey:begin";
const FINISH: &str = "/kernel/v1/auth/passkey:finish";

/// Signs `dev` up with a new passkey; returns (passkey, user_id, member token).
async fn sign_up(h: &Harness, dev: &TestDevice, token: &str) -> (Passkey, String, String) {
    let begin = call(
        h,
        dev,
        token,
        BEGIN,
        &json!({ "mode": "register", "name": "reader@example.org" }),
    )
    .await;
    assert_eq!(begin.status, StatusCode::OK, "{}", begin.body);
    let options = &begin.body["publicKey"];
    assert_eq!(options["attestation"], "none");
    assert_eq!(
        options["authenticatorSelection"]["userVerification"],
        "required"
    );
    let (pk, cred) = Passkey::create(options, READER);
    let fin = call(
        h,
        dev,
        token,
        FINISH,
        &json!({ "ceremony": begin.body["ceremony"], "credential": cred }),
    )
    .await;
    assert_eq!(fin.status, StatusCode::OK, "{}", fin.body);
    (
        pk,
        fin.body["user_id"].as_str().unwrap().to_owned(),
        fin.body["access_token"].as_str().unwrap().to_owned(),
    )
}

async fn sign_in(h: &Harness, dev: &TestDevice, token: &str, pk: &Passkey) -> common::Response {
    let begin = call(h, dev, token, BEGIN, &json!({ "mode": "authenticate" })).await;
    assert_eq!(begin.status, StatusCode::OK, "{}", begin.body);
    let cred = pk.get(&begin.body["publicKey"], READER);
    call(
        h,
        dev,
        token,
        FINISH,
        &json!({ "ceremony": begin.body["ceremony"], "credential": cred }),
    )
    .await
}

#[tokio::test]
async fn sign_up_on_one_device_then_sign_in_on_another() {
    let h = harness();
    let (a, b) = (TestDevice::new(), TestDevice::new());
    let token_a = register(&h, &a).await;
    let (pk, user, member_a) = sign_up(&h, &a, &token_a).await;
    let claims = h.kernel.tokens.verify(&member_a).unwrap();
    assert_eq!(claims.tier, "member");
    assert_eq!(claims.user_id.map(|u| u.to_string()), Some(user.clone()));

    // Device B signs in with the same (synced) passkey: no username needed.
    let token_b = register(&h, &b).await;
    let fin = sign_in(&h, &b, &token_b, &pk).await;
    assert_eq!(fin.status, StatusCode::OK, "{}", fin.body);
    assert_eq!(fin.body["user_id"], user, "same account on both devices");
    // Re-registering device B's key now yields a member token too.
    let again = register(&h, &b).await;
    assert_eq!(
        h.kernel
            .tokens
            .verify(&again)
            .unwrap()
            .user_id
            .map(|u| u.to_string()),
        Some(user)
    );
}

#[tokio::test]
async fn ceremonies_are_single_use_and_bound_to_the_device() {
    let h = harness();
    let (a, b) = (TestDevice::new(), TestDevice::new());
    let token_a = register(&h, &a).await;
    let token_b = register(&h, &b).await;
    let begin = call(&h, &a, &token_a, BEGIN, &json!({ "mode": "register" })).await;
    let (_, cred) = Passkey::create(&begin.body["publicKey"], READER);
    let body = json!({ "ceremony": begin.body["ceremony"], "credential": cred });
    let stolen = call(&h, &b, &token_b, FINISH, &body).await;
    assert_eq!(
        stolen.status,
        StatusCode::FORBIDDEN,
        "another device's ceremony"
    );
    assert_eq!(
        call(&h, &a, &token_a, FINISH, &body).await.status,
        StatusCode::OK
    );
    let replay = call(&h, &a, &token_a, FINISH, &body).await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert_eq!(replay.body["error_description"], "ceremony already used");
}

#[tokio::test]
async fn wrong_origin_or_tampered_assertions_are_refused() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let begin = call(&h, &dev, &token, BEGIN, &json!({ "mode": "register" })).await;
    let (_, cred) = Passkey::create(&begin.body["publicKey"], "https://phish.example");
    let res = call(
        &h,
        &dev,
        &token,
        FINISH,
        &json!({ "ceremony": begin.body["ceremony"], "credential": cred }),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        res.body["error_description"],
        "clientDataJSON: origin not allowed"
    );

    let (pk, _, member) = sign_up(&h, &dev, &token).await;
    let begin = call(&h, &dev, &member, BEGIN, &json!({ "mode": "authenticate" })).await;
    let mut cred = pk.get(&begin.body["publicKey"], READER);
    cred["response"]["signature"] = Value::String(
        pk.get(&begin.body["publicKey"], READER)["response"]["authenticatorData"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    let res = call(
        &h,
        &dev,
        &member,
        FINISH,
        &json!({ "ceremony": begin.body["ceremony"], "credential": cred }),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);
    assert_eq!(res.body["error_description"], "signature does not verify");
}

#[tokio::test]
async fn a_signed_in_device_cannot_join_another_account_and_accounts_cap_devices() {
    let h = harness();
    let first = TestDevice::new();
    let t = register(&h, &first).await;
    let (pk, _, _) = sign_up(&h, &first, &t).await;

    // A device of user X cannot sign in to user Y.
    let other = TestDevice::new();
    let t_other = register(&h, &other).await;
    let (pk_other, _, member_other) = sign_up(&h, &other, &t_other).await;
    let _ = pk_other;
    let res = sign_in(&h, &other, &member_other, &pk).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);

    // Up to 6 active devices per account.
    for n in 2..=6 {
        let d = TestDevice::new();
        let t = register(&h, &d).await;
        assert_eq!(
            sign_in(&h, &d, &t, &pk).await.status,
            StatusCode::OK,
            "device {n}"
        );
    }
    let seventh = TestDevice::new();
    let t = register(&h, &seventh).await;
    let res = sign_in(&h, &seventh, &t, &pk).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    assert_eq!(
        res.body["error_description"],
        "device limit reached: sign out on another device first"
    );
}
