use super::*;

fn fixture(directory: &Path) -> (Config, Report) {
    let config = Config {
        file_bytes: 1024 * 1024,
        timeout_secs: 1,
        probe_interval_ms: 2000,
        probe_timeout_secs: 60,
        max_in_flight: 4,
        requested_file_lanes: Some(4),
        work_dir: directory.into(),
        report_path: directory.join("performance.json"),
    };
    let report = Report {
        schema_version: 1,
        status: "running",
        phase: "setup",
        measurement_started: false,
        run_id: "deadline-test".into(),
        source_sha: None,
        topology: "test",
        configured_file_lanes: 4,
        participant_hops: None,
        padded: true,
        setup_elapsed_ms: None,
        warmup_rtt_ms: None,
        file: FileReport {
            requested_bytes: config.file_bytes,
            ..Default::default()
        },
        probes: Value::Null,
        echo_send_errors: 0,
        transport: Value::Null,
        errors: vec![],
    };
    (config, report)
}

fn directory() -> PathBuf {
    let mut nonce = [0u8; 16];
    gipny_libcore::crypto::fill_random(&mut nonce);
    std::env::temp_dir().join(format!("gipny-performance-report-{:x?}", nonce))
}

#[tokio::test]
async fn setup_and_measurement_share_one_deadline_and_finalize_loss() {
    let directory = directory();
    let (config, mut report) = fixture(&directory);
    write_report(&config.report_path, &config, &report).unwrap();
    let start = Instant::now();
    let deadline = start + Duration::from_millis(250);
    within(deadline, "setup", async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        Ok(())
    })
    .await
    .unwrap();
    report.phase = "measurement";
    report.measurement_started = true;
    report.participant_hops = Some([3, 3, 3, 3]);
    report.setup_elapsed_ms = Some(millis(start.elapsed()));
    report.warmup_rtt_ms = Some(1);
    write_report(&config.report_path, &config, &report).unwrap();
    let mut probes = Probes::new(Instant::now(), Duration::from_secs(60), 4);
    let id = probes.begin(Instant::now()).unwrap();
    probes.sent(id, true);
    // This work would finish within a newly reset 250 ms budget, but setup has
    // already consumed part of the original budget. It must time out instead.
    let result = within(deadline, "measurement", async {
        tokio::time::sleep(Duration::from_millis(225)).await;
        Ok(())
    })
    .await;
    assert!(result.is_err());
    probes.finish(Instant::now());
    report.status = "failed";
    report.errors.push(result.unwrap_err().to_string());
    report.probes = serde_json::to_value(probes.summary()).unwrap();
    write_report(&config.report_path, &config, &report).unwrap();
    let final_report: Value =
        serde_json::from_slice(&std::fs::read(&config.report_path).unwrap()).unwrap();
    assert_eq!(final_report["status"], "failed");
    assert_eq!(final_report["measurement_started"], true);
    assert_eq!(final_report["probes"]["interrupted"], 1);
    assert!(final_report["file"]["goodput_bytes_per_sec"].is_null());
    assert!(!config.report_path.with_extension("json.tmp").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn exhausted_setup_budget_never_polls_the_next_phase() {
    let mut polled = false;
    let deadline = Instant::now() - Duration::from_secs(1);
    assert!(within(deadline, "next phase", async {
        polled = true;
        Ok(())
    })
    .await
    .is_err());
    assert!(!polled);
}

#[test]
fn checkpoint_replacement_preserves_the_previous_report_until_publication() {
    let directory = directory();
    let (config, mut report) = fixture(&directory);
    write_report(&config.report_path, &config, &report).unwrap();
    std::fs::write(
        config.report_path.with_extension("json.tmp"),
        b"partial JSON",
    )
    .unwrap();
    let previous: Value =
        serde_json::from_slice(&std::fs::read(&config.report_path).unwrap()).unwrap();
    assert_eq!(previous["phase"], "setup");
    report.phase = "measurement";
    report.measurement_started = true;
    write_report(&config.report_path, &config, &report).unwrap();
    let checkpoint: Value =
        serde_json::from_slice(&std::fs::read(&config.report_path).unwrap()).unwrap();
    assert_eq!(checkpoint["status"], "running");
    assert_eq!(checkpoint["measurement_started"], true);
    assert!(!config.report_path.with_extension("json.tmp").exists());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn unsafe_or_conflicting_environment_is_rejected_even_when_set_to_zero() {
    for forbidden in FORBIDDEN_ENV
        .iter()
        .copied()
        .chain(["E2E_AGENT_BIN", "E2E_BIG_FILE_RESTART"])
    {
        assert!(
            validate_environment(|name| name == forbidden).is_err(),
            "{forbidden}"
        );
    }
    assert!(validate_environment(|_| false).is_ok());
}

#[test]
fn limits_reject_invalid_values_instead_of_mislabeling_an_experiment() {
    for raw in ["0", "17", "-1", "word"] {
        assert!(bounded(Some(raw), 4, 1, 16, "inflight").is_err());
    }
    assert_eq!(bounded(None, 4, 1, 16, "inflight").unwrap(), 4);
    assert!(bounded(Some("249"), 2000, 250, 60_000, "interval").is_err());
}

#[test]
fn only_a_complete_matching_file_contributes_unique_goodput_once() {
    let mut report = FileReport {
        requested_bytes: 4,
        ..Default::default()
    };
    assert!(report
        .verify(b"abcd", b"abc", || Duration::from_secs(2))
        .is_err());
    assert!(report
        .verify(b"abcd", b"abce", || Duration::from_secs(2))
        .is_err());
    assert_eq!(report.verified_unique_bytes, 0);
    assert_eq!(report.goodput_bytes_per_sec, None);
    assert!(report.verify(b"abcd", b"abcd", || Duration::ZERO).is_err());
    report
        .verify(b"abcd", b"abcd", || Duration::from_secs(2))
        .unwrap();
    report
        .verify(b"abcd", b"abcd", || Duration::from_secs(3))
        .unwrap();
    assert_eq!(report.verified_unique_bytes, 4);
    assert_eq!(report.elapsed_ms, Some(2000));
    assert_eq!(report.goodput_bytes_per_sec, Some(2.0));
    assert_eq!(report.duplicate_completion_events, 1);
}
