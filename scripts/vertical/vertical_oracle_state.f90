module vertical_oracle_state_mod
  implicit none

  ! Minimal state read by the original FLEXPART
  ! verttransform_ecmwf_heights routine. These names intentionally mirror the
  ! windfields_mod state used by the pinned source routine.
  integer :: nuvzmax = 0
  integer :: nzmax = 0
  integer :: nuvz = 0
  integer :: nwz = 0
  integer :: nz = 0

  real, allocatable :: akz(:)
  real, allocatable :: bkz(:)
  real, allocatable :: aknew(:)
  real, allocatable :: bknew(:)
end module vertical_oracle_state_mod
