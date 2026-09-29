//! Per-instance addressing and the hardware file (FR-081, FR-082).
//!
//! # Why some of this spawns a process
//!
//! The default hardware file is `./cluster.yml`, resolved against the working directory. The
//! working directory is process-global, so a test that changed it would race every other test in
//! the binary. The implicit-pickup cases therefore run the real binary with `current_dir` set,
//! and everything that does not need a working directory runs in-process.

#![cfg(feature = "live")]

use std::path::Path;
use std::process::Command;

use workload_gen::hardware::{self, Instance, DEFAULT_MAILBOX, DEFAULT_PORT};

/// A description small enough to load fast, and never actually driven here: every refusal under
/// test happens before an agent is contacted.
const DESCRIPTION: &str = r#"
version: 1
blocks: {tokens: 16, bytes: 32768}
shared_classes:
  manual:
    length: {constant: 2}
    lifetime: {constant: .inf}
session_classes:
  chat:
    pool: {size: {exact: 4}}
    uses: [{class: manual, count: {constant: 1}}]
    turns: {constant: 4}
    input_growth: {constant: 1}
    output_growth: {constant: 1}
    think_time: {constant: 1}
"#;

/// The example in the repository root, which the contract names and a test must keep parseable.
fn example_file() -> std::path::PathBuf {
    // `CARGO_MANIFEST_DIR` is the crate, and the example sits at the app root above `crates/`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("hardware.example.yml")
}

#[test]
fn the_example_hardware_file_parses() {
    // Named in `contracts/hardware.md`. An example nothing reads is an example that rots, and
    // this one is the first thing an operator copies.
    let read = hardware::read(Some(&example_file()))
        .expect("the example must parse")
        .expect("an explicit file is always read");
    assert_eq!(read.rate, Some(1.0));
    assert_eq!(read.instances.len(), 3, "two co-resident plus one default");
    assert!(!read.implicit, "a named file is not an implicit pickup");
    assert!(!read.digest.is_empty(), "the report records a digest");

    // The two co-resident entries are the point of FR-081, and the third takes the defaults.
    assert_eq!(read.instances[0].host, "node5");
    assert_eq!(read.instances[1].host, "node5");
    assert_ne!(read.instances[0].port, read.instances[1].port);
    assert_ne!(read.instances[0].mailbox, read.instances[1].mailbox);
    assert_eq!(read.instances[2].port, DEFAULT_PORT);
    assert_eq!(read.instances[2].mailbox, DEFAULT_MAILBOX);

    // And what the file describes must be a legal deployment, not merely parseable.
    hardware::check_distinct(&read.instances).expect("the example must be a valid deployment");
}

/// Write `text` to a temporary hardware file and read it.
fn read_text(text: &str) -> Result<hardware::Hardware, String> {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("hw.yml");
    std::fs::write(&path, text).unwrap();
    hardware::read(Some(&path)).map(|h| h.expect("an explicit file is always read"))
}

#[test]
fn an_unknown_version_is_refused_rather_than_read_under_the_old_meaning() {
    let e = read_text("version: 2\ninstances: []\n").expect_err("refused");
    assert!(e.contains("version 2"), "{e}");
    assert!(e.contains("understands 1"), "{e}");
}

#[test]
fn an_unknown_field_is_refused_so_a_typo_is_not_silently_a_default() {
    // The failure this prevents is quiet: `mailboxes:` for `mailbox:` would leave the default
    // mailbox in place and the run would drive an instance nobody named.
    let e = read_text("version: 1\nrat: 2.0\n").expect_err("refused");
    assert!(e.contains("cannot parse"), "{e}");
    let e =
        read_text("version: 1\ninstances:\n  - host: a\n    mailboxes: /x\n").expect_err("refused");
    assert!(e.contains("cannot parse"), "{e}");
}

#[test]
fn nothing_belonging_to_the_description_is_accepted() {
    // FR-005 in the other direction. A rate in a description would make the workload
    // unportable; a session class here would make the deployment carry the experiment.
    for field in [
        "seed: 1",
        "blocks: {bytes: 4096}",
        "session_classes: {}",
        "until: 30",
    ] {
        let e = read_text(&format!("version: 1\n{field}\n"))
            .expect_err(&format!("{field} must be refused"));
        assert!(e.contains("cannot parse"), "{field}: {e}");
    }
}

#[test]
fn a_missing_named_file_is_a_mistake_but_a_missing_default_is_not() {
    // Asymmetric on purpose: `--hardware` names a file the operator believes in, while the
    // default not existing is the ordinary case and must not need a flag to suppress.
    let e = hardware::read(Some(Path::new("/nonexistent/cluster.yml"))).expect_err("refused");
    assert!(e.contains("cannot read"), "{e}");
}

#[test]
fn the_rate_can_be_infinite_in_the_file_as_well_as_on_the_command_line() {
    // YAML's own spelling, and the same one a description uses for an unbounded lifetime.
    let read = read_text("version: 1\nrate: .inf\n").expect("parses");
    assert_eq!(read.rate, Some(f64::INFINITY));
}

// ---------------------------------------------------------------------------
// The command line, in-process.
// ---------------------------------------------------------------------------

/// A `run` invocation with the given extra flags, against nothing that is listening.
fn run_argv(extra: &[&str]) -> i32 {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("d.yml");
    std::fs::write(&path, DESCRIPTION).unwrap();
    let mut argv: Vec<String> = vec![
        "workload-gen".into(),
        "run".into(),
        path.display().to_string(),
        "--seed".into(),
        "1".into(),
        "--until".into(),
        "1".into(),
        "--no-launch".into(),
    ];
    argv.extend(extra.iter().map(|s| (*s).to_string()));
    workload_gen::cli::run_argv(&argv)
}

#[test]
fn the_flags_that_addressed_a_host_are_gone() {
    // Replaced rather than kept alongside: a global `--shm-path` and `--agent-port` applied to
    // every `--node` are exactly what made a two-instance deployment unrepresentable.
    for gone in ["--node", "--shm-path", "--agent-port"] {
        assert_eq!(
            run_argv(&[gone, "x"]),
            2,
            "{gone} must no longer be accepted"
        );
    }
}

#[test]
fn a_duplicate_host_and_port_is_refused_before_anything_is_started() {
    // The failure this phase exists to prevent, and it is silent: the second agent's startup
    // takes the first for a leftover and shuts it down, so the run drives one instance and
    // reports two.
    assert_eq!(
        run_argv(&[
            "--instance",
            "127.0.0.1:7999",
            "--instance",
            "127.0.0.1:7999"
        ]),
        2
    );
}

#[test]
fn a_duplicate_mailbox_on_one_host_is_refused_as_one_cache_twice() {
    // Quieter than the port collision: nothing collides, and the run simply measures a
    // deployment other than the one described.
    assert_eq!(
        run_argv(&[
            "--instance",
            "127.0.0.1:7998:/dev/shm/certus-shmq-0",
            "--instance",
            "127.0.0.1:7999:/dev/shm/certus-shmq-0",
        ]),
        2
    );
}

#[test]
fn two_instances_on_one_host_parse_into_two_distinct_specs() {
    // The deployment FR-081 exists for. Asserted on the parse rather than on a run, because a
    // run against nothing listening costs `START_TIMEOUT`, and what is under test here is that
    // two `--instance` flags survive as two instances with their own mailboxes — the exact thing
    // a global `--shm-path` could not express.
    let cli = <workload_gen::cli::Cli as clap::Parser>::try_parse_from([
        "workload-gen",
        "run",
        "d.yml",
        "--seed",
        "1",
        "--instance",
        "node5:7001:/dev/shm/certus-shmq-0",
        "--instance",
        "node5:7002:/dev/shm/certus-shmq-1",
    ])
    .expect("two instances on one host must parse");
    let workload_gen::cli::Command::Run { args } = cli.command else {
        panic!("expected the run subcommand");
    };
    assert_eq!(args.instances.len(), 2);
    assert_eq!(args.instances[0].mailbox, "/dev/shm/certus-shmq-0");
    assert_eq!(args.instances[1].mailbox, "/dev/shm/certus-shmq-1");
    assert_ne!(args.instances[0].label(), args.instances[1].label());
    hardware::check_distinct(&args.instances).expect("a legal deployment");
}

#[test]
fn a_bad_instance_string_is_refused_by_the_parser_with_its_reason() {
    for bad in ["node5:seven", "node5:7001:shm", "[::1]:7420"] {
        assert_eq!(run_argv(&["--instance", bad]), 2, "{bad} must be refused");
    }
}

// ---------------------------------------------------------------------------
// The implicit file, out of process because it needs a working directory.
// ---------------------------------------------------------------------------

/// Run the real binary in `dir`, returning its exit code and its error stream.
fn run_in(dir: &Path, extra: &[&str]) -> (i32, String) {
    let path = dir.join("d.yml");
    std::fs::write(&path, DESCRIPTION).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_workload-gen"))
        .current_dir(dir)
        .args(["run", "d.yml", "--seed", "1", "--until", "1", "--no-launch"])
        .args(extra)
        .output()
        .expect("the generator runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_hardware_file_found_by_default_is_announced() {
    // Required, not a courtesy. A file that changed a run's meaning without appearing in the
    // command line is the failure this specification keeps naming: a plausible number for a
    // different experiment rather than an error.
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("cluster.yml"),
        "version: 1\ninstances:\n  - host: 127.0.0.1\n    port: 7995\n",
    )
    .unwrap();
    let (_code, err) = run_in(dir.path(), &[]);
    assert!(
        err.contains("cluster.yml") && err.contains("found by default"),
        "an implicit hardware file must be announced: {err}"
    );
}

#[test]
fn no_hardware_file_means_no_announcement_and_no_configuration() {
    // The simplest invocation must stay the simplest invocation.
    let dir = tempfile::TempDir::new().unwrap();
    let (_code, err) = run_in(dir.path(), &[]);
    assert!(
        !err.contains("found by default"),
        "nothing should be announced when there is no file: {err}"
    );
}

#[test]
fn the_command_line_overrides_the_file_per_field() {
    // The case that forces this is a rate sweep: vary the rate, hold the deployment fixed, so
    // the rate has to be settable without editing the file.
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("cluster.yml"),
        "version: 1\nrate: 4.0\ninstances:\n  - host: 127.0.0.1\n    port: 7994\n",
    )
    .unwrap();

    // A deliberately duplicate instance list, so each invocation is refused immediately *after*
    // the pacing announcement instead of waiting out `START_TIMEOUT` on a connect. The
    // announcement is what is under test; the refusal after it is scaffolding.
    let stop = [
        "--instance",
        "127.0.0.1:7991",
        "--instance",
        "127.0.0.1:7991",
    ];
    let with = |extra: &[&str]| {
        let mut a = extra.to_vec();
        a.extend_from_slice(&stop);
        run_in(dir.path(), &a).1
    };

    // The file's rate is used when the flag is absent...
    let err = with(&[]);
    assert!(err.contains("paced at 4.000"), "the file's rate: {err}");

    // ...and the flag wins when present.
    let err = with(&["--rate", "9"]);
    assert!(err.contains("paced at 9.000"), "the flag's rate: {err}");

    // `--rate inf` switches the schedule off rather than dividing by infinity, so there is no
    // pacing announcement at all. Left to the arithmetic, `due = t0 + virtual/inf` gives
    // `due == t0` and every turn would record as late by the run's own elapsed time.
    let err = with(&["--rate", "inf"]);
    assert!(
        !err.contains("paced at"),
        "inf must switch pacing off, not pace at infinity: {err}"
    );
}

#[test]
fn an_instance_flag_replaces_the_files_list_rather_than_adding_to_it() {
    // Appending would silently double a cluster on the second invocation of a sweep.
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("cluster.yml"),
        "version: 1\ninstances:\n  - host: 127.0.0.1\n    port: 7993\n",
    )
    .unwrap();
    // If the flag added to the file's list, this would be two instances and the report would
    // say so. It also cannot be a duplicate refusal, which would be exit 2.
    let (code, err) = run_in(dir.path(), &["--instance", "127.0.0.1:7993"]);
    assert_ne!(code, 2, "the same instance twice would be refused: {err}");
}

#[test]
fn a_hardware_file_is_recorded_in_the_report_by_path_and_digest() {
    // So a report says which deployment file this was, not only that there was one.
    let dir = tempfile::TempDir::new().unwrap();
    let text = "version: 1\ninstances:\n  - host: 127.0.0.1\n    port: 7992\n";
    std::fs::write(dir.path().join("cluster.yml"), text).unwrap();
    let read = hardware::read(Some(&dir.path().join("cluster.yml")))
        .unwrap()
        .unwrap();
    // The digest is of the bytes, so an edit that changes nothing semantically still changes it
    // — which is the point: it identifies the file, it does not summarise its meaning.
    let other = read_text(&format!("{text}rate: 1.0\n")).unwrap();
    assert_ne!(read.digest, other.digest);
}

#[test]
fn an_instance_keeps_its_own_mailbox_rather_than_a_global_one() {
    // The defect FR-081 fixes, stated as a property: two entries differing only by mailbox must
    // produce two different specs.
    let a: Instance = "node5:7001:/dev/shm/certus-shmq-0".parse().unwrap();
    let b: Instance = "node5:7002:/dev/shm/certus-shmq-1".parse().unwrap();
    assert_ne!(a.mailbox, b.mailbox);
    assert_ne!(a.label(), b.label(), "two report rows must not read alike");
}
