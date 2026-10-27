//! Borrowed #90 interval-rate grid handoff for downstream GPU stages (#207).
//!
//! #90 keeps its interval amount/rate products private to [`CanonicalGpuField`]
//! so that metadata cannot be recombined with another field's buffers. A
//! downstream stage that consumes the complete interval-mean rate sequence
//! (`flex_extract` `darain`, #207) needs the whole grid rather than one sampled
//! interval. This view exposes it without copying, reading back or recomputing
//! #90 science: the buffer is the existing `rate_millimeter_per_hour` output,
//! interval-major and x-fastest, `[interval * cell_count + cell]`.

use super::{
    encode_accumulated_intervals_gpu_with_kernel, AccumulatedIntervalKernel,
    AccumulatedIntervalMetadataRecord, CanonicalGpuField, GpuContext, MeteorologyCompositionError,
};
use crate::meteorology::{
    Calendar, FieldId, FieldTime, HorizontalGrid, SignConvention, TemporalKind, Unit,
    VerticalStaggering,
};

/// Device-resident #90 interval-mean rate grid with its validated identity.
///
/// The buffer must not be read before the #90 transform has been encoded with
/// [`CanonicalGpuField::encode_interval_rates`] in the same or an earlier
/// submission. It lives as long as the borrowed source owner.
pub struct IntervalRateGrid<'a> {
    /// #90 handoff rates in mm/h, `[interval * cell_count + cell]`.
    pub rates_mm_per_h: &'a wgpu::Buffer,
    /// Canonical accumulated field identity.
    pub field_id: FieldId,
    /// Canonical source unit of the accumulated totals.
    pub source_unit: Unit,
    /// Canonical sign convention of the accumulated totals.
    pub source_sign: SignConvention,
    /// Source calendar shared by every observation.
    pub calendar: Calendar,
    /// Source horizontal grid; cells are x-fastest.
    pub horizontal_grid: &'a HorizontalGrid,
    /// Number of horizontal cells per interval plane.
    pub cell_count: usize,
    /// Validated #75 interval metadata in physical order.
    pub intervals: Vec<AccumulatedIntervalMetadataRecord>,
    /// Original observation times in source order.
    pub source_times: Vec<FieldTime>,
    /// Hashes of the complete canonical source snapshots in source order.
    pub source_snapshot_sha256: Vec<String>,
}

impl CanonicalGpuField<'_> {
    /// Describe this field's device-resident #90 interval-rate grid.
    ///
    /// # Errors
    /// Rejects fields that are not accumulated surface fields, and sources whose
    /// observations disagree on the calendar.
    pub fn interval_rate_grid(&self) -> Result<IntervalRateGrid<'_>, MeteorologyCompositionError> {
        let source = self
            .accumulation
            .as_ref()
            .ok_or(MeteorologyCompositionError::Unsupported(
                "interval-rate grids exist only for accumulated-since-reset fields",
            ))?;
        let first = &self.members[0];
        let grid = &first.snapshot.horizontal_grid;
        let cell_count = grid.nx * grid.ny;
        if first.field.time.kind != TemporalKind::AccumulatedSinceReset
            || first.field.vertical_staggering != VerticalStaggering::NotApplicable
            || first.field.values.len() != cell_count
        {
            return Err(MeteorologyCompositionError::Unsupported(
                "interval-rate grids require one horizontal surface plane",
            ));
        }
        let calendar = first.field.time.calendar;
        if self
            .members
            .iter()
            .any(|m| m.field.time.calendar != calendar)
        {
            return Err(MeteorologyCompositionError::Incompatible(
                "accumulated observations use different calendars",
            ));
        }
        Ok(IntervalRateGrid {
            rates_mm_per_h: &source.outputs.rate_millimeter_per_hour,
            field_id: self.field_id,
            source_unit: first.field.unit,
            source_sign: first.field.sign,
            calendar,
            horizontal_grid: grid,
            cell_count,
            intervals: source.intervals.iter().copied().map(Into::into).collect(),
            source_times: self.members.iter().map(|m| m.field.time.clone()).collect(),
            source_snapshot_sha256: self
                .members
                .iter()
                .map(|m| m.snapshot_sha256.clone())
                .collect(),
        })
    }

    /// Encode this field's existing #90 transform into the caller's encoder.
    ///
    /// Submits, polls and reads back nothing. The rates described by
    /// [`Self::interval_rate_grid`] are valid for later commands in the same
    /// encoder once this returns.
    ///
    /// # Errors
    /// Rejects another device's source or kernel and forwards #90 encode errors.
    pub fn encode_interval_rates(
        &self,
        ctx: &GpuContext,
        kernel: &AccumulatedIntervalKernel,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), MeteorologyCompositionError> {
        if !self.belongs_to(ctx) {
            return Err(MeteorologyCompositionError::Incompatible(
                "source belongs to another GPU device",
            ));
        }
        let source = self
            .accumulation
            .as_ref()
            .ok_or(MeteorologyCompositionError::Missing("interval resources"))?;
        encode_accumulated_intervals_gpu_with_kernel(
            ctx,
            &source.inputs,
            &source.outputs,
            kernel,
            encoder,
        )?;
        Ok(())
    }
}
