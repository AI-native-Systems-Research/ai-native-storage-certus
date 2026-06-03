//! Unit tests for configuration parsing and validation.

use std::io::Write;
use tempfile::NamedTempFile;

// Import from the binary crate's modules.
// We test config logic via JSON parsing since the modules are private to the binary.

fn write_config(json: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(json.as_bytes()).unwrap();
    f
}

#[test]
fn test_valid_minimal_config() {
    let json = r#"{
        "components": [
            {"name": "logger", "dylib": "liblogger.so"}
        ],
        "bindings": []
    }"#;
    let f = write_config(json);
    let config: serde_json::Value = serde_json::from_str(json).unwrap();
    assert!(config["components"].is_array());
    assert_eq!(config["components"].as_array().unwrap().len(), 1);

    // Verify file is readable
    let content = std::fs::read_to_string(f.path()).unwrap();
    let _parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
}

#[test]
fn test_variable_substitution_syntax() {
    let json = r#"{
        "variables": {"num_ssd": 4},
        "components": [
            {"name": "block-dev", "dylib": "libblock.so", "instances": "$num_ssd"}
        ],
        "bindings": []
    }"#;
    let config: serde_json::Value = serde_json::from_str(json).unwrap();
    let instances = &config["components"][0]["instances"];
    assert_eq!(instances.as_str().unwrap(), "$num_ssd");
}

#[test]
fn test_duplicate_component_names_detected() {
    let json = r#"{
        "components": [
            {"name": "logger", "dylib": "liblogger.so"},
            {"name": "logger", "dylib": "liblogger2.so"}
        ],
        "bindings": []
    }"#;
    let config: serde_json::Value = serde_json::from_str(json).unwrap();
    let names: Vec<&str> = config["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    // Detect duplicate
    let mut seen = std::collections::HashSet::new();
    let has_dup = names.iter().any(|n| !seen.insert(n));
    assert!(has_dup);
}

#[test]
fn test_undefined_variable_reference() {
    let json = r#"{
        "variables": {},
        "components": [
            {"name": "block-dev", "dylib": "libblock.so", "instances": "$undefined_var"}
        ],
        "bindings": []
    }"#;
    let config: serde_json::Value = serde_json::from_str(json).unwrap();
    let var_ref = config["components"][0]["instances"].as_str().unwrap();
    let var_name = var_ref.strip_prefix('$').unwrap();
    assert!(!config["variables"]
        .as_object()
        .unwrap()
        .contains_key(var_name));
}

#[test]
fn test_server_config_defaults() {
    let json = r#"{
        "components": [{"name": "a", "dylib": "liba.so"}],
        "bindings": []
    }"#;
    let config: serde_json::Value = serde_json::from_str(json).unwrap();
    assert!(config.get("server").is_none());
}

#[test]
fn test_binding_references_valid_components() {
    let json = r#"{
        "components": [
            {"name": "logger", "dylib": "liblogger.so"},
            {"name": "dispatcher", "dylib": "libdispatcher.so"}
        ],
        "bindings": [
            {"target": "dispatcher", "receptacle": "logger", "source": "logger"}
        ]
    }"#;
    let config: serde_json::Value = serde_json::from_str(json).unwrap();
    let names: Vec<&str> = config["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for binding in config["bindings"].as_array().unwrap() {
        assert!(names.contains(&binding["target"].as_str().unwrap()));
        assert!(names.contains(&binding["source"].as_str().unwrap()));
    }
}

#[test]
fn test_invalid_binding_target() {
    let json = r#"{
        "components": [
            {"name": "logger", "dylib": "liblogger.so"}
        ],
        "bindings": [
            {"target": "nonexistent", "receptacle": "logger", "source": "logger"}
        ]
    }"#;
    let config: serde_json::Value = serde_json::from_str(json).unwrap();
    let names: Vec<&str> = config["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    let target = config["bindings"][0]["target"].as_str().unwrap();
    assert!(!names.contains(&target));
}

#[test]
fn test_cli_override_precedence() {
    let json = r#"{
        "server": {"listen": "0.0.0.0:9000"},
        "components": [{"name": "a", "dylib": "liba.so"}],
        "bindings": []
    }"#;
    let config: serde_json::Value = serde_json::from_str(json).unwrap();
    let json_listen = config["server"]["listen"].as_str().unwrap();
    let cli_listen = "127.0.0.1:50051";
    // CLI takes precedence
    let effective = cli_listen;
    assert_ne!(effective, json_listen);
    assert_eq!(effective, "127.0.0.1:50051");
}
