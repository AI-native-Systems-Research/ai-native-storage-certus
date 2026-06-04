//! JSON configuration parsing and validation for certus-server-composable.
//!
//! Loads a JSON configuration file declaring components, bindings, variables,
//! and server settings. Validates structural integrity before any component
//! is instantiated.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

/// Top-level configuration document.
#[derive(Debug, Clone, Deserialize)]
pub struct Configuration {
    /// Named integer values for substitution in instance count fields.
    #[serde(default)]
    pub variables: HashMap<String, i64>,

    /// Ordered directories for dylib resolution.
    #[serde(default)]
    pub search_paths: Vec<String>,

    /// Server-level settings (CLI arguments override these).
    #[serde(default)]
    pub server: ServerConfig,

    /// Component declarations.
    pub components: Vec<ComponentSpec>,

    /// Wiring rules between components.
    pub bindings: Vec<BindingRule>,

    /// Optional explicit initialization order override.
    pub init_order: Option<Vec<String>>,
}

/// Server-level parameters. All fields can be overridden by CLI arguments.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServerConfig {
    pub listen: Option<String>,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub device_pci: Option<Vec<String>>,
    pub drive_count: Option<usize>,
    pub memory_tier_size: Option<String>,
    pub format: Option<bool>,
    pub poller_base_cpu: Option<usize>,
}

/// Declares a component type to be loaded and instantiated.
#[derive(Debug, Clone, Deserialize)]
pub struct ComponentSpec {
    /// Unique identifier for this component (referenced in bindings).
    pub name: String,

    /// Filename of the shared library (e.g., "liblogger.so").
    pub dylib: String,

    /// Absolute path override (bypasses search_paths).
    pub path: Option<String>,

    /// Factory symbol name override. If not set, derived from dylib filename.
    /// Use when loading multiple component types from a single dylib.
    pub symbol: Option<String>,

    /// Number of instances to create. Integer or "$variable_name".
    #[serde(default = "default_instances")]
    pub instances: InstanceCount,
}

fn default_instances() -> InstanceCount {
    InstanceCount::Literal(1)
}

/// Instance count: either an integer literal or a variable reference.
#[derive(Debug, Clone)]
pub enum InstanceCount {
    Literal(i64),
    Variable(String),
}

impl<'de> Deserialize<'de> for InstanceCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct InstanceCountVisitor;

        impl<'de> de::Visitor<'de> for InstanceCountVisitor {
            type Value = InstanceCount;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a positive integer or a string starting with '$'")
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(InstanceCount::Literal(v))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(InstanceCount::Literal(v as i64))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                if let Some(var_name) = v.strip_prefix('$') {
                    if var_name.is_empty() {
                        return Err(de::Error::custom("empty variable reference"));
                    }
                    Ok(InstanceCount::Variable(var_name.to_string()))
                } else {
                    Err(de::Error::custom(format!(
                        "string instance count must start with '$', got: {v}"
                    )))
                }
            }
        }

        deserializer.deserialize_any(InstanceCountVisitor)
    }
}

/// Connects one component's provided interface to another's receptacle.
#[derive(Debug, Clone, Deserialize)]
pub struct BindingRule {
    /// Name of the component that has the receptacle.
    pub target: String,
    /// Name of the receptacle slot on the target.
    pub receptacle: String,
    /// Name of the component that provides the interface.
    pub source: String,
}

/// Errors from configuration parsing and validation.
#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    Validation(Vec<String>),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "config I/O error: {e}"),
            Self::Parse(e) => write!(f, "config parse error: {e}"),
            Self::Validation(errors) => {
                writeln!(f, "config validation errors:")?;
                for err in errors {
                    writeln!(f, "  - {err}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Load and parse a configuration file from the given path.
///
/// # Errors
///
/// Returns `ConfigError::Io` if the file cannot be read, or
/// `ConfigError::Parse` if the JSON is malformed.
pub fn load_config(path: &Path) -> Result<Configuration, ConfigError> {
    let content = fs::read_to_string(path).map_err(ConfigError::Io)?;
    let config: Configuration = serde_json::from_str(&content).map_err(ConfigError::Parse)?;
    Ok(config)
}

/// Validate configuration structural integrity.
///
/// Checks:
/// - Component names are unique
/// - All variable references are defined
/// - All binding targets/sources reference defined components
/// - Instance counts resolve to positive integers
///
/// # Errors
///
/// Returns `ConfigError::Validation` with a list of all errors found.
pub fn validate_config(config: &Configuration) -> Result<(), ConfigError> {
    let mut errors = Vec::new();

    // Check component name uniqueness.
    let mut seen_names: HashMap<&str, usize> = HashMap::new();
    for (i, comp) in config.components.iter().enumerate() {
        if let Some(&prev_idx) = seen_names.get(comp.name.as_str()) {
            errors.push(format!(
                "duplicate component name '{}' at index {i} (first at {prev_idx})",
                comp.name
            ));
        } else {
            seen_names.insert(&comp.name, i);
        }
    }

    // Check variable references resolve.
    for comp in &config.components {
        if let InstanceCount::Variable(ref var_name) = comp.instances {
            if !config.variables.contains_key(var_name) {
                errors.push(format!(
                    "component '{}' references undefined variable '${var_name}'",
                    comp.name
                ));
            }
        }
    }

    // Validate resolved instance counts are positive.
    for comp in &config.components {
        let count = resolve_instance_count(&comp.instances, &config.variables);
        match count {
            Some(n) if n <= 0 => {
                errors.push(format!(
                    "component '{}' has non-positive instance count: {n}",
                    comp.name
                ));
            }
            None => {
                // Already reported as undefined variable above.
            }
            _ => {}
        }
    }

    // Check binding references.
    for (i, binding) in config.bindings.iter().enumerate() {
        if !seen_names.contains_key(binding.target.as_str()) {
            errors.push(format!(
                "binding[{i}] target '{}' does not match any component name",
                binding.target
            ));
        }
        if !seen_names.contains_key(binding.source.as_str()) {
            errors.push(format!(
                "binding[{i}] source '{}' does not match any component name",
                binding.source
            ));
        }
    }

    // Check init_order references (if provided).
    if let Some(ref order) = config.init_order {
        for name in order {
            if !seen_names.contains_key(name.as_str()) {
                errors.push(format!("init_order references unknown component '{name}'"));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation(errors))
    }
}

/// Resolve an instance count to a concrete integer value.
pub fn resolve_instance_count(
    count: &InstanceCount,
    variables: &HashMap<String, i64>,
) -> Option<i64> {
    match count {
        InstanceCount::Literal(n) => Some(*n),
        InstanceCount::Variable(name) => variables.get(name).copied(),
    }
}

/// Merge CLI overrides into the server configuration.
/// CLI values take precedence over JSON values.
#[allow(clippy::too_many_arguments)]
pub fn merge_cli_overrides(
    config: &mut ServerConfig,
    listen: Option<&str>,
    device_pci: &[String],
    drive_count: Option<usize>,
    memory_tier_size: Option<&str>,
    format: bool,
    tls_cert: Option<&str>,
    tls_key: Option<&str>,
    poller_base_cpu: Option<usize>,
) {
    if let Some(l) = listen {
        config.listen = Some(l.to_string());
    }
    if !device_pci.is_empty() {
        config.device_pci = Some(device_pci.to_vec());
    }
    if drive_count.is_some() {
        config.drive_count = drive_count;
    }
    if let Some(s) = memory_tier_size {
        config.memory_tier_size = Some(s.to_string());
    }
    if format {
        config.format = Some(true);
    }
    if let Some(c) = tls_cert {
        config.tls_cert = Some(c.to_string());
    }
    if let Some(k) = tls_key {
        config.tls_key = Some(k.to_string());
    }
    if poller_base_cpu.is_some() {
        config.poller_base_cpu = poller_base_cpu;
    }
}
