//! Validation of independently observed forward mass-ledger records.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

use crate::particles::MAX_SPECIES;

/// Issue #38's relative closure ceiling, with zero absolute tolerance.
pub const MASS_LEDGER_RELATIVE_TOLERANCE: f64 = 1e-4;

/// The versioned stable-species forward audit profile.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MassLedgerSchema {
    /// Forward point releases with decay disabled and no initial inventory.
    #[serde(rename = "flexpart-gpu.mass-ledger.v1")]
    ForwardStableV1,
}

/// One run's independently observed mass transfers and checkpoints, in kilograms.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MassLedger {
    /// Contract and envelope used to interpret the records.
    pub schema: MassLedgerSchema,
    /// Species identity and nominal request in particle mass-slot order.
    pub species: Vec<SpeciesRequest>,
    /// Positive duration of a complete synchronization interval.
    pub sync_interval_seconds: u64,
    /// Inclusive final boundary, in seconds since model start.
    pub end_seconds: u64,
    /// Every boundary from startup through the terminal sample.
    pub steps: Vec<LedgerStep>,
}

/// Nominal request before conversion to representable particle masses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeciesRequest {
    /// Run-owned species identifier from the species table.
    pub id: String,
    /// Configured mass for the entire run [kg].
    pub requested_kg: f64,
}

/// Observations at one synchronization boundary and over its outgoing interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerStep {
    /// Model-relative time of the boundary [s].
    pub time_seconds: u64,
    /// One row per configured species in run-owned slot order.
    pub species: Vec<Vec<LedgerObservation>>,
}

/// Physical disposition or read-only checkpoint in the forward schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerPhase {
    /// Transfer airborne mass to wet deposition before release.
    Wet,
    /// Add represented release mass to airborne and injected inventories.
    Release,
    /// Observe the mass available for scheduled concentration sampling.
    Sample,
    /// Transfer the residual of trajectories terminated by horizontal transport.
    HorizontalExit,
    /// Transfer airborne mass to dry deposition after transport.
    Dry,
    /// Transfer the residual of particles reaching the configured age bound only.
    MaximumAgeExit,
    /// Transfer the residual of particles meeting the minimum-mass test only.
    MinimumMassExit,
    /// Transfer the residual once when both model termination tests apply.
    MinimumMassAndMaximumAgeExit,
    /// Observe the closing airborne stock after all due operations.
    Close,
}

/// One species' post-phase airborne mass and the operator's independent credit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerObservation {
    /// The scheduled physical disposition or checkpoint.
    pub phase: LedgerPhase,
    /// One transfer to the named destination [kg]. Checkpoints require zero.
    pub transfer_kg: f64,
    /// Independently observed active particle mass after the phase [kg].
    pub airborne_kg: f64,
}

/// Cumulative run inventory at a validated boundary [kg].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct MassInventory {
    /// Actual mass injected into particles.
    pub injected_kg: f64,
    /// Mass currently carried by active particles.
    pub airborne_kg: f64,
    /// Wet deposition inventory.
    pub wet_kg: f64,
    /// Dry deposition inventory.
    pub dry_kg: f64,
    /// Radioactive loss. The stable v1 profile requires zero.
    pub decayed_kg: f64,
    /// Residual mass on horizontal model exit.
    pub horizontal_exit_kg: f64,
    /// Residual mass on age-only termination.
    pub maximum_age_exit_kg: f64,
    /// Residual mass on minimum-mass-only termination.
    pub minimum_mass_exit_kg: f64,
    /// Residual mass when age and minimum-mass tests both apply.
    pub minimum_mass_and_maximum_age_exit_kg: f64,
}

impl MassInventory {
    pub(crate) fn sum(mut self, other: Self) -> Self {
        self.injected_kg += other.injected_kg;
        self.airborne_kg += other.airborne_kg;
        self.wet_kg += other.wet_kg;
        self.dry_kg += other.dry_kg;
        self.decayed_kg += other.decayed_kg;
        self.horizontal_exit_kg += other.horizontal_exit_kg;
        self.maximum_age_exit_kg += other.maximum_age_exit_kg;
        self.minimum_mass_exit_kg += other.minimum_mass_exit_kg;
        self.minimum_mass_and_maximum_age_exit_kg += other.minimum_mass_and_maximum_age_exit_kg;
        self
    }

    fn closes(self) -> bool {
        let accounted = self.airborne_kg
            + self.wet_kg
            + self.dry_kg
            + self.decayed_kg
            + self.horizontal_exit_kg
            + self.maximum_age_exit_kg
            + self.minimum_mass_exit_kg
            + self.minimum_mass_and_maximum_age_exit_kg;
        closes(self.injected_kg, accounted, self.injected_kg)
    }
}

/// Validated inventories in run-owned slot order and the sum across those slots.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LedgerBalance {
    /// Cumulative inventories for each configured species.
    pub species: Vec<MassInventory>,
    /// Cumulative mass across species. Units remain kilograms.
    pub total: MassInventory,
}

/// Deterministic rejection of the first invalid record in time and species order.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MassLedgerError {
    /// Inventory, clock, coverage, or species metadata is invalid.
    #[error("invalid mass-ledger run metadata or step coverage")]
    Run,
    /// The expected phase is missing, out of order, or has an invalid transfer.
    #[error(
        "mass ledger at {time_seconds}s, species slot {slot}, phase {phase:?}: invalid transition"
    )]
    Transition {
        /// Boundary at which the rejected phase was observed.
        time_seconds: u64,
        /// Zero-based configured species slot.
        slot: usize,
        /// Required phase.
        phase: LedgerPhase,
    },
    /// A named cumulative balance failed the closure criterion.
    #[error("mass ledger at {time_seconds}s: cumulative closure failed at species slot {slot:?}")]
    Closure {
        /// Boundary at which the cumulative balance failed.
        time_seconds: u64,
        /// Species slot, or None for the sum across species.
        slot: Option<usize>,
    },
}

use LedgerPhase::{
    Close, Dry, HorizontalExit, MaximumAgeExit, MinimumMassAndMaximumAgeExit, MinimumMassExit,
    Release, Sample, Wet,
};

const FULL_STEP: &[LedgerPhase] = &[
    Wet,
    Release,
    Sample,
    HorizontalExit,
    Dry,
    MaximumAgeExit,
    MinimumMassExit,
    MinimumMassAndMaximumAgeExit,
    Close,
];

const TERMINAL_BOUNDARY: &[LedgerPhase] = &[Wet, Release, Sample, Close];

impl MassLedger {
    /// Audit complete stable-run records and return a balance for each boundary.
    ///
    /// The caller supplies independent observed masses and operator credits.
    /// This method compares them without adjusting reservoirs or certifying
    /// device execution.
    ///
    /// # Errors
    /// Reports invalid coverage, impossible transfers, and failed balances.
    pub fn audit(&self) -> Result<Vec<LedgerBalance>, MassLedgerError> {
        let mut identities = BTreeSet::new();
        if self.species.is_empty()
            || self.species.len() > MAX_SPECIES
            || !self.species.iter().all(|species| {
                !species.id.trim().is_empty()
                    && identities.insert(&species.id)
                    && valid_mass(species.requested_kg)
            })
            || self.sync_interval_seconds == 0
            || self.steps.last().map(|step| step.time_seconds) != Some(self.end_seconds)
        {
            return Err(MassLedgerError::Run);
        }

        let mut inventories = vec![MassInventory::default(); self.species.len()];
        let mut balances = Vec::with_capacity(self.steps.len());
        let mut expected_time = Some(0);
        for step in &self.steps {
            if Some(step.time_seconds) != expected_time
                || step.time_seconds > self.end_seconds
                || step.species.len() != self.species.len()
            {
                return Err(MassLedgerError::Run);
            }
            expected_time = step.time_seconds.checked_add(self.sync_interval_seconds);
            let phases = if step.time_seconds == self.end_seconds {
                TERMINAL_BOUNDARY
            } else {
                FULL_STEP
            };

            for (slot, (observations, inventory)) in
                step.species.iter().zip(&mut inventories).enumerate()
            {
                audit_species_step(
                    step.time_seconds,
                    slot,
                    self.species[slot].requested_kg,
                    observations,
                    phases,
                    inventory,
                )?;
            }
            let total = inventories
                .iter()
                .copied()
                .fold(MassInventory::default(), MassInventory::sum);
            if !total.closes() {
                return Err(MassLedgerError::Closure {
                    time_seconds: step.time_seconds,
                    slot: None,
                });
            }
            balances.push(LedgerBalance {
                species: inventories.clone(),
                total,
            });
        }
        Ok(balances)
    }
}

fn audit_species_step(
    time_seconds: u64,
    slot: usize,
    requested_kg: f64,
    observations: &[LedgerObservation],
    phases: &[LedgerPhase],
    inventory: &mut MassInventory,
) -> Result<(), MassLedgerError> {
    for (index, &phase) in phases.iter().enumerate() {
        let invalid = || MassLedgerError::Transition {
            time_seconds,
            slot,
            phase,
        };
        let observation = observations.get(index).ok_or_else(invalid)?;
        if phase != observation.phase
            || (phase == Release && requested_kg == 0.0 && observation.transfer_kg != 0.0)
            || !inventory.apply(observation, time_seconds == 0)
            || (phase == Close && observations.len() != phases.len())
        {
            return Err(invalid());
        }
    }
    if !inventory.closes() {
        return Err(MassLedgerError::Closure {
            time_seconds,
            slot: Some(slot),
        });
    }
    Ok(())
}

impl MassInventory {
    fn apply(&mut self, observation: &LedgerObservation, startup: bool) -> bool {
        let before = self.airborne_kg;
        let amount = observation.transfer_kg;
        let after = observation.airborne_kg;
        if !valid_mass(amount) || !valid_mass(after) {
            return false;
        }
        if amount == 0.0 {
            return before - after == 0.0;
        }
        let is_release = observation.phase == Release;
        let (observed, credit) = match observation.phase {
            Release => (after - before, &mut self.injected_kg),
            Wet if !startup => (before - after, &mut self.wet_kg),
            Dry => (before - after, &mut self.dry_kg),
            HorizontalExit => (before - after, &mut self.horizontal_exit_kg),
            MaximumAgeExit => (before - after, &mut self.maximum_age_exit_kg),
            // Checkpoints and startup wet deposition transfer no mass. Original
            // minmassfrac is zero (par_mod.f90:178), so neither minimum-mass
            // retirement category carries residual inventory in this profile.
            Wet | Sample | Close | MinimumMassExit | MinimumMassAndMaximumAgeExit => return false,
        };
        if !is_release && (amount > before || (before - amount == 0.0 && after != 0.0)) {
            return false;
        }
        // Using the whole stock as the reference can hide a missing debit or
        // injection whenever the transfer is small relative to that stock.
        if !closes(amount, observed, amount) {
            return false;
        }
        *credit += amount;
        self.airborne_kg = after;
        true
    }
}

fn valid_mass(mass: f64) -> bool {
    mass.is_finite() && mass >= 0.0
}

fn closes(expected: f64, observed: f64, reference: f64) -> bool {
    valid_mass(expected)
        && valid_mass(observed)
        && if reference == 0.0 {
            expected - observed == 0.0
        } else {
            (expected - observed).abs() / reference <= MASS_LEDGER_RELATIVE_TOLERANCE
        }
}
