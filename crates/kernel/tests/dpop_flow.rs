//! End-to-end DPoP flows against the real router, with a Rust client that
//! behaves exactly like the browser's Vault Worker.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::missing_panics_doc)]

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{ORIGIN, TestDevice, Tweak, harness, register, register_request, self_request};
use sanad_kernel::jose::{access_token_hash, b64url};
use sanad_kernel::unix_now;
use serde_json::{Value, json};

#[tokio::test]
async fn register_then_access_protected_route() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let proof = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&token),
        &Tweak::default(),
    );
    let res = h.send(self_request(&proof, &token, "DPoP")).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.body["dpop_jkt"], dev.jkt());
}

#[tokio::test]
async fn resource_server_nonce_challenge_uses_www_authenticate() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let proof = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        None,
        Some(&token),
        &Tweak::default(),
    );
    let res = h.send(self_request(&proof, &token, "DPoP")).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    let www = res
        .headers
        .get("www-authenticate")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        www.starts_with("DPoP ") && www.contains(r#"error="use_dpop_nonce""#),
        "{www}"
    );
    assert!(res.headers.contains_key("dpop-nonce"));
}

#[tokio::test]
async fn replayed_proof_is_rejected() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let proof = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&token),
        &Tweak::default(),
    );
    assert_eq!(
        h.send(self_request(&proof, &token, "DPoP")).await.status,
        StatusCode::OK
    );
    let again = h.send(self_request(&proof, &token, "DPoP")).await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED);
    assert_eq!(again.body["error"], "invalid_dpop_proof");
}

#[tokio::test]
async fn token_cannot_be_used_with_another_devices_key() {
    let h = harness();
    let alice = TestDevice::new();
    let mallory = TestDevice::new();
    let alice_token = register(&h, &alice).await;
    register(&h, &mallory).await;
    // Mallory stole Alice's token but signs proofs with her own (registered) key.
    let proof = mallory.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&alice_token),
        &Tweak::default(),
    );
    let res = h.send(self_request(&proof, &alice_token, "DPoP")).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(res.body["error"], "invalid_token");
    assert_eq!(
        res.body["error_description"],
        "access token is bound to a different key"
    );
}

#[tokio::test]
async fn dpop_bound_token_is_refused_as_bearer() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let proof = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&token),
        &Tweak::default(),
    );
    let res = h.send(self_request(&proof, &token, "Bearer")).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(res.body["error"], "invalid_token");
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // one row per RFC 9449 rejection case
async fn malformed_or_misbound_proofs_are_rejected() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let cases: Vec<(&str, Tweak)> = vec![
        (
            "alg none",
            Tweak {
                header: Some(|hd| hd["alg"] = "none".into()),
                claims: None,
            },
        ),
        (
            "alg HS256",
            Tweak {
                header: Some(|hd| hd["alg"] = "HS256".into()),
                claims: None,
            },
        ),
        (
            "alg RS256",
            Tweak {
                header: Some(|hd| hd["alg"] = "RS256".into()),
                claims: None,
            },
        ),
        (
            "typ jwt",
            Tweak {
                header: Some(|hd| hd["typ"] = "JWT".into()),
                claims: None,
            },
        ),
        (
            "typ missing",
            Tweak {
                header: Some(|hd| {
                    hd.as_object_mut().unwrap().remove("typ");
                }),
                claims: None,
            },
        ),
        (
            "private key in jwk",
            Tweak {
                header: Some(|hd| hd["jwk"]["d"] = "AAAA".into()),
                claims: None,
            },
        ),
        (
            "wrong htm",
            Tweak {
                header: None,
                claims: Some(|c| c["htm"] = "POST".into()),
            },
        ),
        (
            "wrong htu path",
            Tweak {
                header: None,
                claims: Some(|c| c["htu"] = format!("{ORIGIN}/kernel/v1/other").into()),
            },
        ),
        (
            "wrong htu origin",
            Tweak {
                header: None,
                claims: Some(|c| c["htu"] = "https://evil.test/kernel/v1/devices/self".into()),
            },
        ),
        (
            "stale iat",
            Tweak {
                header: None,
                claims: Some(|c| c["iat"] = (unix_now() - 3600).into()),
            },
        ),
        (
            "future iat",
            Tweak {
                header: None,
                claims: Some(|c| c["iat"] = (unix_now() + 3600).into()),
            },
        ),
        (
            "missing ath",
            Tweak {
                header: None,
                claims: Some(|c| {
                    c.as_object_mut().unwrap().remove("ath");
                }),
            },
        ),
        (
            "wrong ath",
            Tweak {
                header: None,
                claims: Some(|c| c["ath"] = access_token_hash("other").into()),
            },
        ),
        (
            "missing jti",
            Tweak {
                header: None,
                claims: Some(|c| {
                    c.as_object_mut().unwrap().remove("jti");
                }),
            },
        ),
        (
            "oversize jti",
            Tweak {
                header: None,
                claims: Some(|c| c["jti"] = "x".repeat(65).into()),
            },
        ),
    ];
    for (name, tweak) in cases {
        let proof = dev.proof(
            "GET",
            "/kernel/v1/devices/self",
            Some(&h.nonce()),
            Some(&token),
            &tweak,
        );
        let res = h.send(self_request(&proof, &token, "DPoP")).await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{name}: {}", res.body);
        assert_eq!(res.body["error"], "invalid_dpop_proof", "{name}");
    }
}

#[tokio::test]
async fn tampered_signature_and_query_handling() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    // A flipped signature byte fails.
    let mut proof = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&token),
        &Tweak::default(),
    );
    let last = proof.pop().unwrap();
    proof.push(if last == 'A' { 'B' } else { 'A' });
    assert_eq!(
        h.send(self_request(&proof, &token, "DPoP")).await.status,
        StatusCode::UNAUTHORIZED
    );
    // htu comparison ignores query and fragment (RFC 9449 §4.3).
    let tweak = Tweak {
        header: None,
        claims: Some(|c| c["htu"] = format!("{ORIGIN}/kernel/v1/devices/self?x=1#frag").into()),
    };
    let proof = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&token),
        &tweak,
    );
    assert_eq!(
        h.send(self_request(&proof, &token, "DPoP")).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn foreign_nonce_and_duplicate_headers_are_rejected() {
    let h = harness();
    let other = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let proof = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&other.nonce()),
        Some(&token),
        &Tweak::default(),
    );
    let res = h.send(self_request(&proof, &token, "DPoP")).await;
    assert_eq!(
        res.body["error"], "use_dpop_nonce",
        "nonce from another Kernel's secret"
    );

    let p1 = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&token),
        &Tweak::default(),
    );
    let p2 = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&token),
        &Tweak::default(),
    );
    let req = Request::get("/kernel/v1/devices/self")
        .header("authorization", format!("DPoP {token}"))
        .header("dpop", p1)
        .header("dpop", p2)
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoked_devices_lose_access_and_cannot_reregister() {
    let h = harness();
    let dev = TestDevice::new();
    let token = register(&h, &dev).await;
    let id = h.kernel.tokens.verify(&token).unwrap().device_id;
    assert!(h.kernel.devices.revoke(id).await.unwrap());

    let proof = dev.proof(
        "GET",
        "/kernel/v1/devices/self",
        Some(&h.nonce()),
        Some(&token),
        &Tweak::default(),
    );
    let res = h.send(self_request(&proof, &token, "DPoP")).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(res.body["error_description"], "device revoked");

    let body = json!({ "ecdh_public_jwk": dev.ecdh_jwk });
    let proof = dev.proof(
        "POST",
        "/kernel/v1/devices",
        Some(&h.nonce()),
        None,
        &Tweak::default(),
    );
    assert_eq!(
        h.send(register_request(&proof, &body)).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn registration_validates_the_ecdh_key() {
    let h = harness();
    let dev = TestDevice::new();
    let send = |body: Value| {
        let proof = dev.proof(
            "POST",
            "/kernel/v1/devices",
            Some(&h.nonce()),
            None,
            &Tweak::default(),
        );
        h.send(register_request(&proof, &body))
    };
    // Off-curve point (invalid-curve attack on future ECDH key wrapping).
    let bad = json!({ "kty": "EC", "crv": "P-256", "x": b64url(&[1; 32]), "y": b64url(&[2; 32]) });
    assert_eq!(
        send(json!({ "ecdh_public_jwk": bad })).await.status,
        StatusCode::BAD_REQUEST
    );
    // Signing key reused as ECDH key.
    assert_eq!(
        send(json!({ "ecdh_public_jwk": dev.jwk })).await.status,
        StatusCode::BAD_REQUEST
    );
    // Unknown fields are refused, not ignored.
    let res = send(json!({ "ecdh_public_jwk": dev.ecdh_jwk, "is_admin": true })).await;
    assert!(res.status.is_client_error());
    // Re-registration with the same key is idempotent (token recovery).
    let a = send(json!({ "ecdh_public_jwk": dev.ecdh_jwk })).await;
    let b = send(json!({ "ecdh_public_jwk": dev.ecdh_jwk })).await;
    assert_eq!(a.status, StatusCode::CREATED);
    assert_eq!(a.body["device_id"], b.body["device_id"]);
}
