//! Explicit host particle synchronization and configured spatial sorting.

use super::ForwardTimeLoopDriver;
use crate::simulation::timeloop::error::TimeLoopError;

impl ForwardTimeLoopDriver {
    pub(super) async fn sync_store_from_gpu(&mut self) -> Result<(), TimeLoopError> {
        let updated = self
            .particle_buffers
            .download_particles(&self.gpu_context)
            .await?;
        self.particle_store.as_mut_slice().copy_from_slice(&updated);
        self.particle_store.recount_active();
        Ok(())
    }

    pub(super) fn apply_spatial_sort_if_enabled(&mut self) -> Result<(), TimeLoopError> {
        let Some(sort_config) = self.config.spatial_sort else {
            return Ok(());
        };
        if self.step_index % sort_config.interval_steps != 0 {
            return Ok(());
        }

        let reorder = self
            .particle_store
            .sort_spatially(sort_config.sort_options)?;
        if !reorder.is_identity() {
            self.particle_buffers
                .upload_store(&self.gpu_context, &self.particle_store)?;
        }
        Ok(())
    }
}
