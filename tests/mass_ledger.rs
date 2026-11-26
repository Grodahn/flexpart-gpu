use flexpart_gpu::validation::mass_ledger::{
    LedgerPhase, MassInventory, MassLedger, MassLedgerError,
};

const MIXED: &str = include_str!("../fixtures/mass-ledger/mixed-v1.json");

fn read(text: &str) -> MassLedger {
    serde_json::from_str(text).expect("versioned ledger")
}

#[test]
fn test_mass_ledger_analytical_transitions_close_per_species_and_total() {
    let cases = [
        (
            include_str!("../fixtures/mass-ledger/injection-v1.json"),
            16.0,
            16.0,
            0.0,
            0.0,
            0.0,
        ),
        (
            include_str!("../fixtures/mass-ledger/wet-v1.json"),
            12.0,
            9.0,
            3.0,
            0.0,
            0.0,
        ),
        (
            include_str!("../fixtures/mass-ledger/dry-v1.json"),
            12.0,
            9.0,
            0.0,
            3.0,
            0.0,
        ),
        (
            include_str!("../fixtures/mass-ledger/exit-v1.json"),
            12.0,
            9.0,
            0.0,
            0.0,
            3.0,
        ),
    ];
    for (text, injected_kg, airborne_kg, wet_kg, dry_kg, horizontal_exit_kg) in cases {
        let balances = read(text).audit().expect("closes");
        let expected = MassInventory {
            injected_kg,
            airborne_kg,
            wet_kg,
            dry_kg,
            horizontal_exit_kg,
            ..MassInventory::default()
        };
        assert_eq!(balances[1].species, [expected]);
        assert_eq!(balances[1].total, expected);
    }

    let balances = read(MIXED).audit().expect("mixed balances");
    let expected_a = MassInventory {
        injected_kg: 16.0,
        airborne_kg: 6.0,
        wet_kg: 3.0,
        dry_kg: 1.0,
        horizontal_exit_kg: 2.0,
        maximum_age_exit_kg: 4.0,
        ..MassInventory::default()
    };
    let expected_b = MassInventory {
        injected_kg: 8.0,
        airborne_kg: 4.0,
        wet_kg: 2.0,
        dry_kg: 2.0,
        ..MassInventory::default()
    };
    assert_eq!(balances[1].species, [expected_a, expected_b]);
    assert_eq!(
        balances[1].total,
        MassInventory {
            injected_kg: 24.0,
            airborne_kg: 10.0,
            wet_kg: 5.0,
            dry_kg: 3.0,
            horizontal_exit_kg: 2.0,
            maximum_age_exit_kg: 4.0,
            ..MassInventory::default()
        }
    );
    assert_eq!(balances[1], balances[2]);
    let text = serde_json::to_string(&read(MIXED)).expect("serialize");
    assert_eq!(read(&text).audit().expect("round-trip audit"), balances);
}

#[test]
fn test_mass_ledger_malformed_or_incomplete_run_is_rejected() {
    for text in [
        MIXED.replace("ledger.v1", "ledger.v2"),
        MIXED.replace("horizontal_exit", "other_exit"),
        MIXED.replace("transfer_kg", "unknown_kg"),
        MIXED.replace("requested_kg", "requested_g"),
    ] {
        assert!(serde_json::from_str::<MassLedger>(&text).is_err());
    }
    let mut ledger = read(MIXED);
    ledger.species[0].requested_kg = 0.0;
    assert_eq!(
        ledger.audit(),
        Err(MassLedgerError::Transition {
            time_seconds: 0,
            slot: 0,
            phase: LedgerPhase::Release,
        })
    );
    let mut ledger = read(MIXED);
    ledger.steps.remove(1);
    assert_eq!(ledger.audit(), Err(MassLedgerError::Run));
    let mut ledger = read(MIXED);
    ledger.species[1].id = ledger.species[0].id.clone();
    assert_eq!(ledger.audit(), Err(MassLedgerError::Run));
}

#[test]
fn test_mass_ledger_wrong_phase_and_losses_fail_at_first_observation() {
    use LedgerPhase::{Dry, HorizontalExit, MinimumMassExit, Release, Sample, Wet};
    let expect = |ledger: &MassLedger, phase| {
        assert_eq!(
            ledger.audit(),
            Err(MassLedgerError::Transition {
                time_seconds: 60,
                slot: 0,
                phase,
            })
        );
    };
    let mut ledger = read(MIXED);
    ledger.steps[1].species[0].swap(0, 1);
    expect(&ledger, Wet);

    let mut ledger = read(MIXED);
    ledger.steps[1].species[0][0].airborne_kg = -0.000_001;
    expect(&ledger, Wet);

    let mut ledger = read(MIXED);
    ledger.steps[1].species[0][1].airborne_kg = f64::NAN;
    expect(&ledger, Release);

    let mut ledger = read(MIXED);
    ledger.steps[1].species[0][2].airborne_kg -= 0.000_001;
    expect(&ledger, Sample);

    let mut ledger = read(MIXED);
    ledger.steps[1].species[0][3].transfer_kg = 14.0;
    expect(&ledger, HorizontalExit);

    let mut ledger = read(MIXED);
    ledger.steps[1].species[0][3].transfer_kg = 13.0;
    ledger.steps[1].species[0][3].airborne_kg = 0.000_001;
    expect(&ledger, HorizontalExit);

    let mut ledger = read(MIXED);
    ledger.steps[1].species[0][4].airborne_kg -= 1.0;
    expect(&ledger, Dry);

    let mut ledger = read(MIXED);
    ledger.steps[1].species[0][6].transfer_kg = 1.0;
    ledger.steps[1].species[0][6].airborne_kg -= 1.0;
    expect(&ledger, MinimumMassExit);
}

#[test]
fn test_mass_ledger_small_local_errors_cannot_cancel_between_species() {
    let mut ledger = read(MIXED);
    // Individually admissible local differences accumulate above the cumulative
    // bound. Leave the endpoint consistent so the cumulative check must catch it.
    for (observation, offset) in ledger.steps[1].species[0].iter_mut().zip([
        0.001, 0.002, 0.002, 0.003, 0.003, 0.003, 0.003, 0.003, 0.003,
    ]) {
        observation.airborne_kg += offset;
    }
    for (observation, offset) in ledger.steps[1].species[1].iter_mut().zip([
        0.0007, 0.0007, 0.0007, 0.0007, 0.0012, 0.0012, 0.0012, 0.0012, 0.0012,
    ]) {
        observation.airborne_kg -= offset;
    }
    assert_eq!(
        ledger.audit(),
        Err(MassLedgerError::Closure {
            time_seconds: 60,
            slot: Some(0),
        })
    );
}
