//! Species forcing shapes, materialization and upload-cache semantics.

use crate::particles::MAX_SPECIES;
use crate::simulation::timeloop::error::TimeLoopError;

/// Per-particle forcing input shape for one timestep.
#[derive(Debug, Clone)]
pub enum ParticleForcingField {
    /// Use one scalar for all particle slots.
    Uniform(f32),
    /// Explicit per-slot values; length must match particle-buffer slot count.
    PerParticle(Vec<f32>),
}

impl Default for ParticleForcingField {
    fn default() -> Self {
        Self::Uniform(0.0)
    }
}

impl ParticleForcingField {
    pub(super) fn materialize_into<'a>(
        &'a self,
        expected_len: usize,
        field: &'static str,
        scratch: &'a mut Vec<f32>,
        cached_uniform_value: &mut Option<f32>,
    ) -> Result<Option<&'a [f32]>, TimeLoopError> {
        match self {
            Self::Uniform(value) => {
                if cached_uniform_value.is_some_and(|prev| prev.to_bits() == value.to_bits())
                    && scratch.len() == expected_len
                {
                    return Ok(None);
                }
                if scratch.len() != expected_len {
                    scratch.resize(expected_len, 0.0);
                }
                scratch.fill(*value);
                *cached_uniform_value = Some(*value);
                Ok(Some(scratch.as_slice()))
            }
            Self::PerParticle(values) => {
                if values.len() != expected_len {
                    return Err(TimeLoopError::ForcingLengthMismatch {
                        field,
                        expected: expected_len,
                        actual: values.len(),
                    });
                }
                *cached_uniform_value = None;
                Ok(Some(values.as_slice()))
            }
        }
    }

    /// Returns `true` when the forcing is uniformly zero, meaning the
    /// corresponding deposition process has no effect and can be skipped.
    #[must_use]
    pub(super) fn is_zero(&self) -> bool {
        matches!(self, Self::Uniform(v) if *v == 0.0)
    }
}

/// Timestep forcing fields consumed by deposition, decay, and Langevin updates.
///
/// Deposition and decay forcing are per-species: entry `s` of each vector
/// applies to particle mass slot `s` (see [`MAX_SPECIES`]). All per-species
/// vectors must have the same length in `1..=MAX_SPECIES`; GPU uploads pad
/// unused lanes with zeros.
#[derive(Debug, Clone)]
pub struct ForwardStepForcing {
    /// Dry deposition velocity `vdep` [m/s] per species.
    pub dry_deposition_velocity_m_s: Vec<ParticleForcingField>,
    /// Wet scavenging coefficient `lambda` [1/s] per species.
    pub wet_scavenging_coefficient_s_inv: Vec<ParticleForcingField>,
    /// Wet precipitating-area fraction [0..1], shared across species.
    ///
    /// The precipitating sub-grid area is geometric (see `get_wetscav.f90`
    /// `grfraction` logic) and identical for all species of one particle.
    pub wet_precipitating_fraction: ParticleForcingField,
    /// Radioactive decay constant `lambda` [1/s] per species, uniform across
    /// particles. Zero means stable.
    pub decay_constant_s_inv: Vec<f32>,
    /// Density-gradient term `(1/rho) * d(rho)/dz` [1/m] for Langevin vertical drift.
    pub rho_grad_over_rho: f32,
}

impl Default for ForwardStepForcing {
    fn default() -> Self {
        Self {
            dry_deposition_velocity_m_s: vec![ParticleForcingField::Uniform(0.0)],
            wet_scavenging_coefficient_s_inv: vec![ParticleForcingField::Uniform(0.0)],
            wet_precipitating_fraction: ParticleForcingField::Uniform(0.0),
            decay_constant_s_inv: vec![0.0],
            rho_grad_over_rho: 0.0,
        }
    }
}

impl ForwardStepForcing {
    /// Number of forced species slots.
    #[must_use]
    pub fn species_count(&self) -> usize {
        self.dry_deposition_velocity_m_s.len()
    }

    /// Validate per-species vector shapes for one timestep.
    ///
    /// All per-species vectors must agree in length within `1..=MAX_SPECIES`.
    pub(super) fn check_species_shape(&self) -> Result<usize, TimeLoopError> {
        let count = self.dry_deposition_velocity_m_s.len();
        if count == 0 || count > MAX_SPECIES {
            return Err(TimeLoopError::ForcingLengthMismatch {
                field: "dry_deposition_velocity_m_s",
                expected: MAX_SPECIES,
                actual: count,
            });
        }
        if self.wet_scavenging_coefficient_s_inv.len() != count {
            return Err(TimeLoopError::ForcingLengthMismatch {
                field: "wet_scavenging_coefficient_s_inv",
                expected: count,
                actual: self.wet_scavenging_coefficient_s_inv.len(),
            });
        }
        if self.decay_constant_s_inv.len() != count {
            return Err(TimeLoopError::ForcingLengthMismatch {
                field: "decay_constant_s_inv",
                expected: count,
                actual: self.decay_constant_s_inv.len(),
            });
        }
        Ok(count)
    }

    /// Whether dry deposition can be skipped (all species uniformly zero).
    #[must_use]
    pub(super) fn skip_dry_deposition(&self) -> bool {
        self.dry_deposition_velocity_m_s
            .iter()
            .all(ParticleForcingField::is_zero)
    }

    /// Whether wet deposition can be skipped.
    #[must_use]
    pub(super) fn skip_wet_deposition(&self) -> bool {
        self.wet_precipitating_fraction.is_zero()
            || self
                .wet_scavenging_coefficient_s_inv
                .iter()
                .all(ParticleForcingField::is_zero)
    }

    /// Whether radioactive decay can be skipped (all species stable).
    #[must_use]
    pub(super) fn skip_decay(&self) -> bool {
        self.decay_constant_s_inv
            .iter()
            .all(|lambda| !lambda.is_finite() || *lambda <= 0.0)
    }

    /// Decay constants padded to [`MAX_SPECIES`] lanes for GPU uniform upload.
    #[must_use]
    pub(super) fn decay_lanes(&self) -> [f32; MAX_SPECIES] {
        let mut lanes = [0.0_f32; MAX_SPECIES];
        for (slot, lambda) in self
            .decay_constant_s_inv
            .iter()
            .enumerate()
            .take(MAX_SPECIES)
        {
            lanes[slot] = lambda.max(0.0);
        }
        lanes
    }
}

/// Materialize per-species forcing into an interleaved flat buffer.
///
/// Layout is particle-major: `scratch[slot * MAX_SPECIES + lane]`. Each entry
/// of `fields` covers one species lane; missing lanes (fewer entries than
/// [`MAX_SPECIES`]) are zero-filled. Uniform fast path: when every entry is
/// [`ParticleForcingField::Uniform`] and matches the cached lanes, the scratch
/// is already current and `Ok(None)` skips the GPU upload.
pub(super) fn materialize_species_forcing_into<'a>(
    fields: &'a [ParticleForcingField],
    expected_slots: usize,
    field: &'static str,
    scratch: &'a mut Vec<f32>,
    cached_uniform_lanes: &mut Option<[f32; MAX_SPECIES]>,
) -> Result<Option<&'a [f32]>, TimeLoopError> {
    let mut uniform_lanes: [f32; MAX_SPECIES] = [0.0; MAX_SPECIES];
    let mut all_uniform = true;
    for (lane, entry) in fields.iter().enumerate() {
        match entry {
            ParticleForcingField::Uniform(value) => uniform_lanes[lane] = *value,
            ParticleForcingField::PerParticle(_) => all_uniform = false,
        }
    }
    let lane_count = fields.len();
    debug_assert!(lane_count <= MAX_SPECIES);

    if all_uniform {
        // Bitwise lane comparison: exact replay of the same uniform values
        // skips the GPU upload; any bit difference re-uploads (conservative).
        let lanes_unchanged = cached_uniform_lanes
            .is_some_and(|prev| prev.map(f32::to_bits) == uniform_lanes.map(f32::to_bits));
        if lanes_unchanged && scratch.len() == expected_slots * MAX_SPECIES {
            return Ok(None);
        }
        if scratch.len() != expected_slots * MAX_SPECIES {
            scratch.resize(expected_slots * MAX_SPECIES, 0.0);
        }
        for slot in 0..expected_slots {
            let base = slot * MAX_SPECIES;
            scratch[base..base + MAX_SPECIES].copy_from_slice(&uniform_lanes);
        }
        *cached_uniform_lanes = Some(uniform_lanes);
        return Ok(Some(scratch.as_slice()));
    }

    *cached_uniform_lanes = None;
    if scratch.len() != expected_slots * MAX_SPECIES {
        scratch.resize(expected_slots * MAX_SPECIES, 0.0);
    }
    for (lane, entry) in fields.iter().enumerate() {
        match entry {
            ParticleForcingField::Uniform(value) => {
                for slot in 0..expected_slots {
                    scratch[slot * MAX_SPECIES + lane] = *value;
                }
            }
            ParticleForcingField::PerParticle(values) => {
                if values.len() != expected_slots {
                    return Err(TimeLoopError::ForcingLengthMismatch {
                        field,
                        expected: expected_slots,
                        actual: values.len(),
                    });
                }
                for (slot, value) in values.iter().enumerate() {
                    scratch[slot * MAX_SPECIES + lane] = *value;
                }
            }
        }
    }
    // Zero lanes beyond the forced species count (scratch may be reused).
    for slot in 0..expected_slots {
        for lane in lane_count..MAX_SPECIES {
            scratch[slot * MAX_SPECIES + lane] = 0.0;
        }
    }
    Ok(Some(scratch.as_slice()))
}

/// Reinterpret interleaved species forcing lanes as fixed lane arrays.
///
/// `flat` must hold `slot_count * MAX_SPECIES` values in
/// `slot * MAX_SPECIES + lane` order (see
/// [`materialize_species_forcing_into`]).
pub(super) fn cast_species_lanes(
    flat: &[f32],
    slot_count: usize,
) -> Result<&[[f32; MAX_SPECIES]], TimeLoopError> {
    if flat.len() != slot_count * MAX_SPECIES {
        return Err(TimeLoopError::ForcingLengthMismatch {
            field: "species_forcing_lanes",
            expected: slot_count * MAX_SPECIES,
            actual: flat.len(),
        });
    }
    Ok(bytemuck::cast_slice(flat))
}
