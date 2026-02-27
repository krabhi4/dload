// Env-var tests mutate the global process environment and MUST run sequentially.
// Cargo runs integration tests in separate processes by default (one process per test file),
// so tests within this file are safe as long as they run single-threaded.
// We force single-threaded execution via the non-#[tokio::test] attribute.

use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn dload_port_env_overrides_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("DLOAD_PORT", "9090");
    std::env::remove_var("DLOAD_HOST");
    let cfg = dload::config::RuntimeConfig::from_env();
    std::env::remove_var("DLOAD_PORT");
    assert_eq!(cfg.port, 9090);
}

#[test]
fn dload_host_env_overrides_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("DLOAD_HOST", "127.0.0.1");
    std::env::remove_var("DLOAD_PORT");
    let cfg = dload::config::RuntimeConfig::from_env();
    std::env::remove_var("DLOAD_HOST");
    assert_eq!(cfg.host, "127.0.0.1");
}

#[test]
fn dload_config_defaults() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("DLOAD_PORT");
    std::env::remove_var("DLOAD_HOST");
    let cfg = dload::config::RuntimeConfig::from_env();
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 8080);
}

#[test]
fn dload_port_invalid_falls_back_to_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("DLOAD_PORT", "not-a-port");
    std::env::remove_var("DLOAD_HOST");
    let cfg = dload::config::RuntimeConfig::from_env();
    std::env::remove_var("DLOAD_PORT");
    assert_eq!(cfg.port, 8080);
}
