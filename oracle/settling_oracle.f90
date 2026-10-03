! Direct pinned FLEXPART 11.1 routine driver for issue #35.
! No settling, slip, viscosity or drag equations are reimplemented here.
! Crosswalk: settling_mod.f90:93-289 (get_settling, including its sphere branch),
! drydepo_mod.f90:607-728 (part0), readoptions_mod.f90:2337-2352
! (single-bin species initialization), par_mod.f90:61,192 (ga,maxndia).
! Constant two-level meteorology supplies the local air state exactly at
! the lower level; spatial interpolation parity is outside this claim.
program settling_oracle
  use par_mod, only: maxndia
  use com_mod, only: density, dquer, vsetaver, cunningham, ishape
  use windfields_mod, only: nz, height, tt, rho
  use drydepo_mod, only: part0
  use settling_mod, only: get_settling
  implicit none
  character(len=1024) :: input_path, output_path
  character(len=32) :: vector_id
  integer :: in_unit, out_unit, ios
  real :: diameter_um, particle_density, temperature, air_density, settling, cun
  real :: fractions(maxndia), schmidt(maxndia), stokes(maxndia)

  if (command_argument_count() /= 2) error stop 'expected input and output paths'
  if (maxndia /= 1) error stop 'issue #35 requires the pinned single-bin configuration'
  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)
  allocate(density(1), dquer(1), vsetaver(1), cunningham(1), ishape(1))
  allocate(height(2), tt(0:0,0:0,2,1), rho(0:0,0:0,2,1))
  nz = 2
  height = [0.0, 100.0]
  ishape = 0
  open(newunit=in_unit, file=trim(input_path), status='old', action='read')
  open(newunit=out_unit, file=trim(output_path), status='replace', action='write')
  do
    read(in_unit, *, iostat=ios) vector_id, diameter_um, particle_density, temperature, air_density
    if (ios < 0) exit
    if (ios /= 0) error stop 'malformed oracle input'
    dquer(1) = diameter_um
    density(1) = particle_density
    tt = temperature
    rho = air_density
    ! PDSIGMA=2 is a valid #10 carrier width; maxndia=1 leaves one mass bin.
    call part0(dquer(1), 2.0, density(1), maxndia, fractions, schmidt, cun, stokes)
    vsetaver(1) = -sum(stokes * fractions)
    cunningham(1) = cun * sum(fractions)
    call get_settling(0.0, 0.0, 0.0, 1, settling)
    write(out_unit, '(A,1X,ES24.16)') trim(vector_id), settling
  end do
  close(in_unit)
  close(out_unit)
end program settling_oracle
