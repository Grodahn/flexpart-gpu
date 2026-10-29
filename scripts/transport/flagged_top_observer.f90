! Observation-only GNU linker wrappers. Every argument is forwarded unchanged
! to the genuine pinned routine; no particle, flag, forcing or status is written.
module flagged_top_observer
  use iso_c_binding, only: c_int, c_float
  use iso_fortran_env, only: int32, int64
  use particle_mod, only: part
  use com_mod, only: lsettling
  use interpol_mod, only: w
  implicit none
  integer :: observation_unit, observation_id=0, stage_clock=0, movement=0
contains
  subroutine emit_observation(stage)
    character(len=*), intent(in) :: stage
    if (observation_id==0) return
    write(observation_unit,'(A,1X,I0,1X,A,1X,I0,5(1X,ES25.17E3),2(1X,L1),3(1X,Z16.16),2(1X,Z8.8))') &
      'OBS',observation_id,stage,stage_clock,part(1)%xlon,part(1)%ylat,part(1)%z, &
      real(part(1)%settling,kind(part(1)%z)),real(w,kind(part(1)%z)),lsettling,part(1)%nstop, &
      transfer(part(1)%xlon,0_int64),transfer(part(1)%ylat,0_int64),transfer(part(1)%z,0_int64), &
      transfer(part(1)%settling,0_int32),transfer(w,0_int32)
  end subroutine
end module

subroutine observe_current(itime,x,y,z,eta) bind(C,name='__wrap___interpol_mod_MOD_interpol_wind')
  use flagged_top_observer
  implicit none
  integer(c_int), intent(in) :: itime
  real(c_float), intent(in) :: x,y,z,eta
  interface
    subroutine original(itime,x,y,z,eta) bind(C,name='__real___interpol_mod_MOD_interpol_wind')
      import c_int,c_float
      integer(c_int), intent(in) :: itime
      real(c_float), intent(in) :: x,y,z,eta
    end subroutine
  end interface
  stage_clock=itime; movement=0
  call emit_observation('CURRENT_IN')
  call original(itime,x,y,z,eta)
  call emit_observation('CURRENT_OUT')
end subroutine

subroutine observe_advanced(itime,x,y,z,eta) bind(C,name='__wrap___interpol_mod_MOD_interpol_wind_short')
  use flagged_top_observer
  implicit none
  integer(c_int), intent(in) :: itime
  real(c_float), intent(in) :: x,y,z,eta
  interface
    subroutine original(itime,x,y,z,eta) bind(C,name='__real___interpol_mod_MOD_interpol_wind_short')
      import c_int,c_float
      integer(c_int), intent(in) :: itime
      real(c_float), intent(in) :: x,y,z,eta
    end subroutine
  end interface
  stage_clock=itime
  call emit_observation('ADVANCED_IN')
  call original(itime,x,y,z,eta)
  call emit_observation('ADVANCED_OUT')
end subroutine

subroutine observe_move(ipart,delta) bind(C,name='__wrap___particle_mod_MOD_update_z_sp')
  use flagged_top_observer
  implicit none
  integer(c_int), intent(in) :: ipart
  real(c_float), intent(in) :: delta
  interface
    subroutine original(ipart,delta) bind(C,name='__real___particle_mod_MOD_update_z_sp')
      import c_int,c_float
      integer(c_int), intent(in) :: ipart
      real(c_float), intent(in) :: delta
    end subroutine
  end interface
  call original(ipart,delta)
  movement=movement+1
  if (movement==1) then
    call emit_observation('PREDICTOR_RAW')
  else
    call emit_observation('CORRECTOR_RAW')
  endif
end subroutine

subroutine observe_place(ipart,value) bind(C,name='__wrap___particle_mod_MOD_set_z_sp')
  use flagged_top_observer
  implicit none
  integer(c_int), intent(in) :: ipart
  real(c_float), intent(in) :: value
  interface
    subroutine original(ipart,value) bind(C,name='__real___particle_mod_MOD_set_z_sp')
      import c_int,c_float
      integer(c_int), intent(in) :: ipart
      real(c_float), intent(in) :: value
    end subroutine
  end interface
  call original(ipart,value)
  if (movement==1) then
    call emit_observation('PLACE_PREDICTOR')
  else
    call emit_observation('PLACE_CORRECTOR')
  endif
end subroutine
