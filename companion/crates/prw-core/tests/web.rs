//! Properties of `prw web`'s rules: which group a PR goes in, and who the server answers.

use proptest::prelude::*;
use prw_core::status::PillKind;
use prw_core::web::{Asked, Bucket, Denied, Route, authorize, bucket_of, route, same_secret};

const PORT: u16 = 7420;
const SECRET: &str = "0123456789abcdef";

fn pill() -> impl Strategy<Value = PillKind> {
    prop::sample::select(PillKind::ALL.to_vec())
}

proptest! {
    #[test]
    fn a_pr_is_done_exactly_when_merged_or_closed(
        pill in pill(),
        is_yours: bool,
        is_asked: bool,
    ) {
        let bucket = bucket_of(pill, is_yours, is_asked);
        prop_assert_eq!(
            bucket == Bucket::Done,
            matches!(pill, PillKind::Merged | PillKind::Closed)
        );
        // Your review asked for is yours to do, once the PR is read and still open.
        if is_asked && bucket != Bucket::Done && pill != PillKind::Loading {
            prop_assert_eq!(bucket, Bucket::You);
        }
        // Not read yet: nothing to do about it.
        if pill == PillKind::Loading {
            prop_assert_eq!(bucket, Bucket::Waiting);
        }
        // Someone else's PR, your review not asked: never yours to act on.
        if !is_yours && !is_asked {
            prop_assert_ne!(bucket, Bucket::You);
        }
    }

    #[test]
    fn no_other_host_is_answered(host in "[a-z0-9.:]{0,24}", key in "[0-9a-f]{0,16}") {
        let asked = Asked { host: Some(&host), key: Some(&key), ..Asked::default() };
        let is_ours = host == format!("127.0.0.1:{PORT}") || host == format!("localhost:{PORT}");
        for route in [Route::State, Route::Asset("index.html".into())] {
            let answer = authorize(&route, &asked, PORT, SECRET);
            if !is_ours {
                prop_assert_eq!(answer, Err(Denied::Host));
            }
        }
    }

    #[test]
    fn the_api_takes_the_secret_alone(key in "[0-9a-f]{0,20}") {
        let host = format!("127.0.0.1:{PORT}");
        let asked = Asked { host: Some(&host), key: Some(&key), ..Asked::default() };
        let answer = authorize(&Route::State, &asked, PORT, SECRET);
        prop_assert_eq!(answer.is_ok(), key == SECRET);
        prop_assert_eq!(same_secret(&key, SECRET), key == SECRET);
    }

    #[test]
    fn every_request_has_one_route(method in "GET|POST|PUT|HEAD", path in "/[a-z./]{0,20}") {
        let routed = route(&method, &path);
        // Only the page's files and the API are answered; nothing climbs out of the build.
        if let Route::Asset(file) = &routed {
            prop_assert!(!file.split('/').any(|part| part == ".."));
            prop_assert!(!file.starts_with("api/"));
        }
        if path.starts_with("/api/") {
            prop_assert!(!matches!(routed, Route::Asset(_)));
        }
    }
}
