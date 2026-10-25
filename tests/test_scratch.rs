#[allow(dead_code)]
mod common;

use std::thread;

#[test]
fn per_test_fixtures_are_removed_when_the_test_thread_exits() {
    let (homes, key) = thread::spawn(|| {
        let first = common::data_home_path();
        let second = common::data_home_path();
        let key = common::maintainer_key_path();
        assert!(first.is_dir() && second.is_dir() && key.is_file());
        assert_ne!(first, second, "each command must get a fresh data home");
        assert_eq!(key, common::maintainer_key_path(), "key fixture is reused");
        ([first, second], key)
    })
    .join()
    .expect("fixture thread");

    for path in homes.iter().chain([&key]) {
        assert!(!path.exists(), "test fixture leaked: {}", path.display());
    }
}
