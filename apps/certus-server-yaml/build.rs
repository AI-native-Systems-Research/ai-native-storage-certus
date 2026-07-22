use std::collections::{HashMap, HashSet};
use std::fmt::Write as FmtWrite;
use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};

use serde::Deserialize;

// --- YAML schema types ---

#[derive(Deserialize)]
struct ProfileManifest {
    profile: ProfileMeta,
    components: HashMap<String, ComponentDecl>,
    wiring: Vec<WiringEntry>,
    init_order: Vec<String>,
    exports: Vec<ExportEntry>,
}

#[derive(Deserialize)]
struct ProfileMeta {
    name: String,
    description: String,
}

#[derive(Deserialize)]
struct ComponentDecl {
    #[serde(rename = "crate")]
    crate_name: String,
    factory: String,
    provides: Vec<String>,
    /// Override the Rust module path for the provided trait(s).
    /// Defaults to "interfaces" if not specified.
    trait_path: Option<String>,
    init_hook: Option<String>,
    /// "factory" means this component is created N times at runtime (not a singleton).
    /// It generates a factory closure instead of a singleton instance.
    kind: Option<String>,
    /// Receptacles that the factory-kind component needs wired.
    /// Used to generate the correct factory closure body.
    #[serde(default)]
    receptacles: Vec<String>,
}

#[derive(Deserialize)]
struct WiringEntry {
    target: String,
    source: String,
    /// Which interface of the source component to wire. Defaults to the source's
    /// first `provides` entry; specify explicitly when a source provides several
    /// interfaces (e.g. a responder exposing both `IRemoteLookupRdmaResponder`
    /// and `IRemoteLookupRdmaResponderAdmin`).
    #[serde(default)]
    interface: Option<String>,
}

#[derive(Deserialize)]
struct ExportEntry {
    component: String,
    interface: String,
}

// --- Validation ---

fn validate_manifest(manifest: &ProfileManifest) {
    let component_names: HashSet<&str> = manifest.components.keys().map(|s| s.as_str()).collect();

    // Validate wiring references
    for entry in &manifest.wiring {
        let parts: Vec<&str> = entry.target.split('.').collect();
        if parts.len() != 2 {
            panic!(
                "invalid wiring target '{}': expected 'component.receptacle'",
                entry.target
            );
        }
        if !component_names.contains(parts[0]) {
            panic!("wiring target references unknown component '{}'", parts[0]);
        }
        if !component_names.contains(entry.source.as_str()) {
            panic!(
                "wiring source references unknown component '{}'",
                entry.source
            );
        }
    }

    // Validate init_order covers all components with hooks
    let init_set: HashSet<&str> = manifest.init_order.iter().map(|s| s.as_str()).collect();
    for (name, decl) in &manifest.components {
        if decl.init_hook.is_some() && !init_set.contains(name.as_str()) {
            panic!(
                "component '{}' has init_hook but is missing from init_order",
                name
            );
        }
        if !init_set.contains(name.as_str()) && decl.init_hook.is_none() {
            // OK — components without hooks don't need to be in init_order
        }
    }

    // Validate init_order only references declared components
    for name in &manifest.init_order {
        if !component_names.contains(name.as_str()) {
            panic!("init_order references unknown component '{}'", name);
        }
    }

    // Validate exports
    for export in &manifest.exports {
        if !component_names.contains(export.component.as_str()) {
            panic!("export references unknown component '{}'", export.component);
        }
        let decl = &manifest.components[&export.component];
        if !decl.provides.contains(&export.interface) {
            panic!(
                "export interface '{}' not in component '{}' provides list",
                export.interface, export.component
            );
        }
    }
}

// --- Code generation ---

fn rust_crate_ident(crate_name: &str) -> String {
    crate_name.replace('-', "_")
}

fn interface_to_trait(iface: &str) -> String {
    iface.to_string()
}

fn ensure_callable(factory: &str) -> String {
    let trimmed = factory.trim();
    if trimmed.ends_with(')') {
        trimmed.to_string()
    } else {
        format!("{trimmed}()")
    }
}

fn is_factory_kind(decl: &ComponentDecl) -> bool {
    decl.kind.as_deref() == Some("factory")
}

fn generate_composition(manifest: &ProfileManifest) -> String {
    let mut code = String::new();

    writeln!(
        code,
        "// AUTO-GENERATED from profiles/{}.yaml — do not edit manually",
        manifest.profile.name
    )
    .unwrap();
    writeln!(
        code,
        "// Profile: {} — {}",
        manifest.profile.name, manifest.profile.description
    )
    .unwrap();
    writeln!(code).unwrap();
    writeln!(code, "use component_core::query_interface;").unwrap();
    writeln!(code).unwrap();

    // Generate ComponentStack struct
    writeln!(code, "pub struct ComponentStack {{").unwrap();
    for export in &manifest.exports {
        let trait_name = interface_to_trait(&export.interface);
        let export_decl = &manifest.components[&export.component];
        let trait_mod = export_decl.trait_path.as_deref().unwrap_or("interfaces");
        let trait_mod = rust_crate_ident(trait_mod);
        writeln!(
            code,
            "    pub {}: std::sync::Arc<dyn {}::{} + Send + Sync>,",
            export.component, trait_mod, trait_name
        )
        .unwrap();
    }
    // Eviction channel receiver (from dispatcher component)
    let dispatcher_crate = rust_crate_ident(&manifest.components["dispatcher"].crate_name);
    writeln!(code, "    pub eviction_rx: crossbeam_channel::Receiver<{dispatcher_crate}::EvictionEvent>,").unwrap();
    writeln!(code, "    pub eviction_dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,").unwrap();
    writeln!(code, "}}").unwrap();
    writeln!(code).unwrap();

    // Generate build_stack function
    writeln!(code, "#[allow(unused_imports, unused_variables)]").unwrap();
    writeln!(
        code,
        "pub fn build_stack(config: &crate::config::StackConfig) -> Result<ComponentStack, String> {{"
    )
    .unwrap();

    // --- Instantiation phase (skip factory-kind components) ---
    writeln!(code, "    // --- Instantiate components ---").unwrap();
    for name in &manifest.init_order {
        let decl = &manifest.components[name];
        if is_factory_kind(decl) {
            continue;
        }
        let crate_ident = rust_crate_ident(&decl.crate_name);
        let factory_call = ensure_callable(&decl.factory);
        writeln!(code, "    let comp_{name} = {crate_ident}::{factory_call};",).unwrap();
    }
    // Instantiate components not in init_order (those without hooks, skip factories)
    for (name, decl) in &manifest.components {
        if !manifest.init_order.contains(name) && !is_factory_kind(decl) {
            let crate_ident = rust_crate_ident(&decl.crate_name);
            let factory_call = ensure_callable(&decl.factory);
            writeln!(code, "    let comp_{name} = {crate_ident}::{factory_call};",).unwrap();
        }
    }
    writeln!(code).unwrap();

    // --- Query interfaces phase (skip factory-kind) ---
    writeln!(code, "    // --- Query interfaces ---").unwrap();
    for (name, decl) in &manifest.components {
        if is_factory_kind(decl) {
            continue;
        }
        let trait_mod = decl.trait_path.as_deref().unwrap_or("interfaces");
        let trait_mod = rust_crate_ident(trait_mod);
        for iface in &decl.provides {
            let trait_name = interface_to_trait(iface);
            let var_name = format!(
                "iface_{}_{}",
                name,
                iface.to_lowercase().trim_start_matches('i')
            );
            writeln!(
                code,
                "    let {var_name}: std::sync::Arc<dyn {trait_mod}::{trait_name} + Send + Sync> = query_interface!(comp_{name}, {trait_mod}::{trait_name})"
            )
            .unwrap();
            writeln!(
                code,
                "        .ok_or(\"failed to query {trait_name} from {name}\")?;"
            )
            .unwrap();
        }
    }
    writeln!(code).unwrap();

    // --- Wiring phase ---
    writeln!(code, "    // --- Wire receptacles ---").unwrap();
    for entry in &manifest.wiring {
        let parts: Vec<&str> = entry.target.split('.').collect();
        let target_comp = parts[0];
        let receptacle = parts[1];
        let source_comp = &entry.source;

        // Interface to wire: the entry's explicit choice, else the source's
        // first provided interface.
        let source_decl = &manifest.components[source_comp];
        let source_iface = entry.interface.as_ref().unwrap_or(&source_decl.provides[0]);
        let iface_var = format!(
            "iface_{}_{}",
            source_comp,
            source_iface.to_lowercase().trim_start_matches('i')
        );

        writeln!(
            code,
            "    comp_{target_comp}.{receptacle}.connect(std::sync::Arc::clone(&{iface_var}))"
        )
        .unwrap();
        writeln!(
            code,
            "        .map_err(|e| format!(\"{target_comp}.{receptacle}: {{e}}\"))?;",
        )
        .unwrap();
    }
    writeln!(code).unwrap();

    // --- Factory injection phase (for kind: factory components) ---
    let factory_components: Vec<(&String, &ComponentDecl)> = manifest
        .components
        .iter()
        .filter(|(_, decl)| is_factory_kind(decl))
        .collect();

    if !factory_components.is_empty() {
        writeln!(code, "    // --- Inject component factories ---").unwrap();
        for (name, decl) in &factory_components {
            let crate_ident = rust_crate_ident(&decl.crate_name);
            let factory_call = ensure_callable(&decl.factory);

            if *name == "block_device" {
                if decl.crate_name == "block-device-kernel" {
                    // Kernel backend: capture device_paths from config for runtime indexing
                    writeln!(
                        code,
                        "    let __device_paths = config.device_paths.clone();"
                    )
                    .unwrap();
                    writeln!(code, "    comp_dispatcher.set_block_device_factory(Box::new(move |spdk_env, logger, drive_idx, pci_addr, cpu_pin| {{").unwrap();
                    writeln!(
                        code,
                        "        let path = if drive_idx < __device_paths.len() {{"
                    )
                    .unwrap();
                    writeln!(code, "            __device_paths[drive_idx].clone()").unwrap();
                    writeln!(code, "        }} else {{").unwrap();
                    writeln!(code, "            format!(\"/dev/nvme{{}}n1\", drive_idx)").unwrap();
                    writeln!(code, "        }};").unwrap();
                    writeln!(code, "        let bd = {crate_ident}::BlockDeviceKernelComponent::create(&path, 4096, 0);").unwrap();
                } else if decl.crate_name == "block-device-filesys" {
                    // Filesys backend: capture device_paths from config for runtime indexing
                    writeln!(
                        code,
                        "    let __device_paths = config.device_paths.clone();"
                    )
                    .unwrap();
                    writeln!(code, "    comp_dispatcher.set_block_device_factory(Box::new(move |spdk_env, logger, drive_idx, pci_addr, cpu_pin| {{").unwrap();
                    writeln!(
                        code,
                        "        let path = if drive_idx < __device_paths.len() {{"
                    )
                    .unwrap();
                    writeln!(code, "            __device_paths[drive_idx].clone()").unwrap();
                    writeln!(code, "        }} else {{").unwrap();
                    writeln!(
                        code,
                        "            format!(\"/ssd/certus-drive-{{}}.img\", drive_idx)"
                    )
                    .unwrap();
                    writeln!(code, "        }};").unwrap();
                    writeln!(code, "        let bd = {crate_ident}::BlockDeviceFilesysComponent::create(&path, 4096, 4194304);").unwrap();
                } else {
                    writeln!(code, "    comp_dispatcher.set_block_device_factory(Box::new(|spdk_env, logger, drive_idx, pci_addr, cpu_pin| {{").unwrap();
                    writeln!(code, "        let bd = {crate_ident}::{factory_call};").unwrap();
                }
                for recep in &decl.receptacles {
                    match recep.as_str() {
                        "spdk_env" => {
                            writeln!(code, "        bd.spdk_env.connect(std::sync::Arc::clone(spdk_env)).map_err(|e| e.to_string())?;").unwrap();
                        }
                        "logger" => {
                            writeln!(code, "        bd.logger.connect(std::sync::Arc::clone(logger)).map_err(|e| e.to_string())?;").unwrap();
                        }
                        _ => {
                            panic!("unsupported block_device receptacle '{recep}' in profile");
                        }
                    }
                }
                writeln!(code, "        let admin: std::sync::Arc<dyn interfaces::IBlockDeviceAdmin + Send + Sync> = component_core::query_interface!(bd, interfaces::IBlockDeviceAdmin)").unwrap();
                writeln!(code, "            .ok_or_else(|| \"failed to query IBlockDeviceAdmin\".to_string())?;").unwrap();
                if decl.crate_name == "block-device-spdk-nvme" {
                    // SPDK backend: use IBlockDeviceAdmin for PCI-based init
                    writeln!(code, "        admin.set_pci_address(pci_addr);").unwrap();
                    writeln!(code, "        if let Some(cpu) = cpu_pin {{ admin.set_actor_cpu(cpu + drive_idx); }}").unwrap();
                    writeln!(
                        code,
                        "        admin.initialize().map_err(|e| e.to_string())?;"
                    )
                    .unwrap();
                } else if decl.crate_name == "block-device-kernel" {
                    // Kernel block device backend: initialize via io_uring
                    writeln!(code, "        bd.initialize().map_err(|e| e.to_string())?;").unwrap();
                } else {
                    // Non-SPDK backend: call initialize() directly on the component
                    writeln!(code, "        bd.initialize().map_err(|e| e.to_string())?;").unwrap();
                }
                writeln!(code, "        let ibd: std::sync::Arc<dyn interfaces::IBlockDevice + Send + Sync> = component_core::query_interface!(bd, interfaces::IBlockDevice)").unwrap();
                writeln!(code, "            .ok_or_else(|| \"failed to query IBlockDevice from factory component\".to_string())?;").unwrap();
                writeln!(code, "        Ok((bd as std::sync::Arc<dyn component_core::IUnknown + Send + Sync>, ibd, admin))").unwrap();
                writeln!(code, "    }}));").unwrap();
            } else if *name == "extent_manager" {
                writeln!(code, "    comp_dispatcher.set_extent_manager_factory(Box::new(|logger, dma_alloc| {{").unwrap();
                writeln!(code, "        let em = {crate_ident}::{factory_call};").unwrap();
                writeln!(code, "        em.set_dma_alloc(dma_alloc);").unwrap();
                writeln!(code, "        em.logger.connect(std::sync::Arc::clone(logger) as std::sync::Arc<dyn interfaces::ILogger + Send + Sync>).unwrap();").unwrap();
                writeln!(
                    code,
                    "        em as std::sync::Arc<dyn component_core::IUnknown + Send + Sync>"
                )
                .unwrap();
                writeln!(code, "    }}));").unwrap();
            }
        }
        writeln!(code).unwrap();
    }

    // --- Initialization phase ---
    writeln!(code, "    // --- Initialize (in declared order) ---").unwrap();
    for name in &manifest.init_order {
        let decl = &manifest.components[name];
        if is_factory_kind(decl) {
            continue;
        }
        if let Some(hook) = &decl.init_hook {
            let iface = &decl.provides[0];
            let iface_var = format!(
                "iface_{}_{}",
                name,
                iface.to_lowercase().trim_start_matches('i')
            );
            writeln!(code, "    crate::hooks::{hook}(&{iface_var}, config)?;").unwrap();
        }
    }
    writeln!(code).unwrap();

    // --- Create eviction channel from dispatcher component ---
    writeln!(code, "    let eviction_rx = comp_dispatcher.create_eviction_channel(16384);").unwrap();
    writeln!(code, "    let eviction_dropped = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));").unwrap();
    writeln!(code).unwrap();

    // --- Return exports ---
    writeln!(code, "    Ok(ComponentStack {{").unwrap();
    for export in &manifest.exports {
        let iface_var = format!(
            "iface_{}_{}",
            export.component,
            export.interface.to_lowercase().trim_start_matches('i')
        );
        writeln!(code, "        {}: {iface_var},", export.component).unwrap();
    }
    writeln!(code, "        eviction_rx,").unwrap();
    writeln!(code, "        eviction_dropped,").unwrap();
    writeln!(code, "    }})").unwrap();
    writeln!(code, "}}").unwrap();

    code
}

// --- Protoc helpers (from certus-server) ---

const PROTOC_VERSION: &str = "25.1";

fn find_protoc() -> Option<PathBuf> {
    if let Ok(p) = env::var("PROTOC") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(output) = Command::new("which").arg("protoc").output() {
        if output.status.success() {
            let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

fn download_protoc() -> PathBuf {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let protoc_dir = out_dir.join("protoc");
    let protoc_bin = protoc_dir.join("bin").join("protoc");

    if protoc_bin.exists() {
        return protoc_bin;
    }

    let url = format!(
        "https://github.com/protocolbuffers/protobuf/releases/download/v{}/protoc-{}-linux-x86_64.zip",
        PROTOC_VERSION, PROTOC_VERSION
    );

    let zip_path = out_dir.join("protoc.zip");

    let status = Command::new("curl")
        .args(["-sL", "-o"])
        .arg(&zip_path)
        .arg(&url)
        .status()
        .expect("failed to run curl");
    assert!(status.success(), "failed to download protoc from {url}");

    fs::create_dir_all(&protoc_dir).unwrap();
    let status = Command::new("unzip")
        .args(["-q", "-o"])
        .arg(&zip_path)
        .arg("-d")
        .arg(&protoc_dir)
        .status()
        .expect("failed to run unzip");
    assert!(status.success(), "failed to unzip protoc");

    fs::remove_file(&zip_path).ok();
    protoc_bin
}

// --- Main ---

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Determine profile
    let profile = env::var("CERTUS_PROFILE").unwrap_or_else(|_| "full".into());
    let manifest_path = format!("profiles/{profile}.yaml");

    println!("cargo:rerun-if-changed={manifest_path}");
    println!("cargo:rerun-if-env-changed=CERTUS_PROFILE");
    println!("cargo:rerun-if-changed=proto/dispatcher.proto");

    // 2. Parse and validate manifest
    let yaml_content = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("cannot read profile '{manifest_path}': {e}"));
    let manifest: ProfileManifest = serde_yaml::from_str(&yaml_content)
        .unwrap_or_else(|e| panic!("invalid YAML in '{manifest_path}': {e}"));
    validate_manifest(&manifest);

    // Check that required features are enabled for the profile's crates
    for (name, decl) in &manifest.components {
        match decl.crate_name.as_str() {
            "block-device-filesys" => {
                if env::var("CARGO_FEATURE_FILESYS").is_err() {
                    panic!(
                        "profile '{profile}' uses block-device-filesys (component '{name}') \
                         but the 'filesys' feature is not enabled.\n\
                         Build with: CERTUS_PROFILE={profile} cargo build -p certus-server-yaml \
                         --features filesys --no-default-features"
                    );
                }
            }
            "block-device-spdk-nvme" => {
                if env::var("CARGO_FEATURE_SPDK").is_err() {
                    panic!(
                        "profile '{profile}' uses block-device-spdk-nvme (component '{name}') \
                         but the 'spdk' feature is not enabled.\n\
                         Build with: CERTUS_PROFILE={profile} cargo build -p certus-server-yaml \
                         --features spdk"
                    );
                }
            }
            "block-device-kernel" => {
                if env::var("CARGO_FEATURE_KERNEL").is_err() {
                    panic!(
                        "profile '{profile}' uses block-device-kernel (component '{name}') \
                         but the 'kernel' feature is not enabled.\n\
                         Build with: CERTUS_PROFILE={profile} cargo build -p certus-server-yaml \
                         --features kernel --no-default-features"
                    );
                }
            }
            _ => {}
        }
    }

    // 3. Generate composition code
    let composition_code = generate_composition(&manifest);
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("composition.rs"), &composition_code)?;

    // 4. Compile protobuf
    let protoc = find_protoc().unwrap_or_else(|| {
        eprintln!("cargo:warning=protoc not found, downloading v{PROTOC_VERSION}...");
        download_protoc()
    });
    env::set_var("PROTOC", &protoc);
    tonic_build::compile_protos("proto/dispatcher.proto")?;

    Ok(())
}
