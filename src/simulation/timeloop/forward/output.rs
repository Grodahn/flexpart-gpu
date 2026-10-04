//! Explicit caller-requested GPU concentration output boundary.

use super::ForwardTimeLoopDriver;
use crate::gpu::{
    accumulate_concentration_grid_gpu, ConcentrationGridOutput, ConcentrationGridShape,
    ConcentrationGriddingParams,
};
use crate::simulation::timeloop::error::TimeLoopError;

impl ForwardTimeLoopDriver {
    /// Accumulate current particle state into a concentration grid on GPU.
    ///
    /// If a GPU submission is still pending from the last timestep, call
    /// [`finalize`](Self::finalize) first to ensure particle positions are
    /// up to date.
    pub async fn accumulate_concentration_grid(
        &self,
        shape: ConcentrationGridShape,
        params: ConcentrationGriddingParams,
    ) -> Result<ConcentrationGridOutput, TimeLoopError> {
        accumulate_concentration_grid_gpu(&self.gpu_context, &self.particle_buffers, shape, params)
            .await
            .map_err(Into::into)
    }
}
