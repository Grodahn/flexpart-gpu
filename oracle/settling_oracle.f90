! Pinned FLEXPART 11.1 gravitational settling oracle harness for issue #35.
!
! Implements exactly the spherical (`ishape == 0`) path of
! `settling_mod.f90:get_settling` with single-bin initialization from
! `drydepo_mod.f90:part0` and constants from `par_mod.f90`, at the pinned
! revision c70586c2b7f5258850705325881c61f557ea9bd8 (see
! `reference/flexpart-11.1.json`).
!
! Crosswalk (FLEXPART 11.1, pinned commit):
! - `par_mod.f90:61`: `ga = 9.81`
! - `drydepo_mod.f90:part0` (`subroutine part0`, lines 607-728):
!   `myl = 1.81e-5`, `lam = 6.53e-8`, `eps = 1.2e-38`,
!   `Kn = 2*lam/dmean`, `alpha = 1.257 + 0.4*exp(-1.1/Kn)` with the
!   `(-1.1/Kn) <= log10(eps)*log(10)` guard, `cun = 1 + alpha*Kn`,
!   `vsh = ga*density*dmean^2*cun/(18*myl)`.
!   Single diameter bin (`maxndia = 1`, `par_mod.f90:192`; `ndia = maxndia`,
!   `readoptions_mod.f90:3097`) gives `dmean = dquer/1e6`,
!   `cunningham = cun`, `vsetaver = -vset`.
! - `readoptions_mod.f90:2341-2349`: `dquer` m-to-um conversion and the
!   `part0`/`vsetaver`/`cunningham` initialization loop.
! - `settling_mod.f90:viscosity`: Sutherland `c = 120`, `t_0 = 291.15`,
!   `eta_0 = 1.827e-5`.
! - `settling_mod.f90:get_settling` sphere branch: `vis_kin = vis_dyn/airdens`,
!   `reynolds = dquer/1e6*abs(vsetaver)/vis_kin`, up to 20 iterations of the
!   Clift-Gauvin drag (`24/Re` for `Re <= 0.02`, otherwise
!   `24/Re*(1+0.15*Re**0.687) + 0.42/(1+42500/Re**1.16)`) and
!   `settling = -sqrt(4*ga*dquer/1e6*density*cunningham/(3*c_d*airdens))`,
!   exiting when `abs((settling-settling_old)/settling) < 0.01`.
!
! This harness bypasses only the `xt/yt/zt` grid interpolation
! (`settling_mod.f90` lines 162-200): #35 canonical vectors supply local air
! temperature and density directly, which is the documented #35 scope
! (carrier/meteorology inputs, units, regime transitions, validity bounds).
! Spatial interpolation remains owned by #87-#90/#76 and is not claimed here.
!
! Usage: ./settling_oracle <input_tsv> <output_tsv>
! Input TSV columns: vector_id diameter_um density_kg_m3 temperature_K air_density_kg_m3
! Output TSV columns: vector_id settling_velocity_m_s (ES24.16, negative downward)
program settling_oracle
  implicit none
  character(len=256) :: input_path, output_path
  integer :: in_unit, out_unit, ios
  character(len=32) :: vector_id
  real :: diameter_um, density, temperature, airdens, settling

  if (command_argument_count() /= 2) then
    write(*,*) 'usage: settling_oracle <input_tsv> <output_tsv>'
    error stop 1
  end if
  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)

  open(newunit=in_unit, file=trim(input_path), status='old', action='read', iostat=ios)
  if (ios /= 0) error stop 2
  open(newunit=out_unit, file=trim(output_path), status='replace', action='write', iostat=ios)
  if (ios /= 0) error stop 3

  do
    read(in_unit, *, iostat=ios) vector_id, diameter_um, density, temperature, airdens
    if (ios /= 0) exit
    call settling_sphere(diameter_um, density, temperature, airdens, settling)
    write(out_unit, '(A,1X,ES24.16)') trim(vector_id), settling
  end do

  close(in_unit)
  close(out_unit)

contains

  subroutine settling_sphere(dquer_um, rho_p, temp_k, rho_air, settling)
    real, intent(in) :: dquer_um, rho_p, temp_k, rho_air
    real, intent(out) :: settling
    real, parameter :: ga = 9.81
    real, parameter :: myl = 1.81e-5
    real, parameter :: lam = 6.53e-8
    real, parameter :: eps = 1.2e-38
    real, parameter :: eta0 = 1.827e-5, t0 = 291.15, c_suth = 120.0
    real :: d_m, kn, alpha, cun, vsh, vsetaver
    real :: vis_dyn, vis_kin, reynolds, c_d, settling_old
    integer :: i

    d_m = dquer_um / 1.0e6

    kn = 2.0 * lam / d_m
    if ((-1.1 / kn) <= log10(eps) * log(10.0)) then
      alpha = 1.257
    else
      alpha = 1.257 + 0.4 * exp(-1.1 / kn)
    end if
    cun = 1.0 + alpha * kn
    vsh = ga * rho_p * d_m * d_m * cun / (18.0 * myl)
    vsetaver = -vsh

    vis_dyn = eta0 * (t0 + c_suth) / (temp_k + c_suth) * (temp_k / t0) ** 1.5
    vis_kin = vis_dyn / rho_air

    reynolds = d_m * abs(vsetaver) / vis_kin
    settling_old = vsetaver

    do i = 1, 20
      if (reynolds <= 0.02) then
        c_d = 24.0 / reynolds
      else
        c_d = (24.0 / reynolds) * (1.0 + 0.15 * (reynolds ** 0.687)) + &
              0.42 / (1.0 + 42500.0 / (reynolds ** 1.16))
      end if

      settling = -1.0 * sqrt(4.0 * ga * d_m * rho_p * cun / (3.0 * c_d * rho_air))

      if (abs((settling - settling_old) / settling) < 0.01) exit

      reynolds = d_m * abs(settling) / vis_kin
      settling_old = settling
    end do
  end subroutine settling_sphere

end program settling_oracle
