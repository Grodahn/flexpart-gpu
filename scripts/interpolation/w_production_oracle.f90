program w_production_oracle
  ! Direct issue #80 oracle for the pristine FLEXPART 11.1 eta=no W path.
  ! The linked upstream routines perform both native-interface remapping and
  ! public production sampling; this driver only initializes their module state.
  use par_mod, only: numwfmem
  use com_mod, only: memtime, memind, numbnests, xglobal, sglobal, nglobal, &
    lcw, lcwsum, ipin, loutrestart
  use point_mod, only: grid_dx => dx, grid_dy => dy, &
    grid_xlon0 => xlon0, grid_ylat0 => ylat0
  use windfields_mod, only: nxmax, nymax, nuvzmax, nwzmax, nzmax, nx, ny, &
    nz, nxfield, nxmin1, nymin1, nuvz, nwz, dxconst, dyconst, height, &
    akm, bkm, akz, bkz, aknew, bknew, tt2, td2, ps, tth, qvh, &
    etawheight, ww, alloc_windfields
  use verttransform_mod, only: verttransform_init, &
    verttransform_ecmwf_heights, verttransform_ecmwf_windfields
  use interpol_mod, only: interpol_wind, sampled_w => w
  implicit none

  integer, parameter :: grid_nx = 2
  integer, parameter :: grid_ny = 2
  integer :: native_nz, input_unit, output_unit
  integer :: k, level, memory, nquery, query, query_kind
  real :: surface_pressure, temperature_2m, dewpoint_2m
  real :: fraction, particle_height
  real, allocatable :: input_a(:), input_b(:), input_temperature(:)
  real, allocatable :: input_humidity(:), input_omega(:)
  real, allocatable :: uuh(:, :, :), vvh(:, :, :), wwh(:, :, :), pvh(:, :, :)
  real, allocatable :: rhoh(:, :, :), prsh(:, :, :), pinmconv(:, :, :)
  real, allocatable :: uvzlev(:, :, :)
  character(len=1024) :: input_path, output_path

  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)
  if (len_trim(input_path) == 0 .or. len_trim(output_path) == 0) then
    error stop "usage: w_production_oracle <input.txt> <output.txt>"
  endif

  open(newunit=input_unit, file=trim(input_path), status="old", action="read")
  read(input_unit, *) native_nz
  if (native_nz < 2) error stop "at least two native layers are required"

  allocate(input_a(native_nz + 1), input_b(native_nz + 1))
  allocate(input_temperature(native_nz), input_humidity(native_nz))
  allocate(input_omega(native_nz + 1))
  read(input_unit, *) surface_pressure, temperature_2m, dewpoint_2m
  do k = 1, native_nz + 1
    read(input_unit, *) input_a(k), input_b(k)
  enddo
  do k = 1, native_nz
    read(input_unit, *) input_temperature(k), input_humidity(k)
  enddo
  do k = 1, native_nz + 1
    read(input_unit, *) input_omega(k)
  enddo
  read(input_unit, *) nquery
  if (nquery < 3) error stop "lower, interior, and upper samples are required"

  call configure_flexpart(native_nz)
  call alloc_windfields
  call populate_vertical_state(native_nz, surface_pressure, temperature_2m, &
    dewpoint_2m, input_a, input_b, input_temperature, input_humidity)

  allocate(uuh(0:grid_nx - 1, 0:grid_ny - 1, nuvzmax))
  allocate(vvh(0:grid_nx - 1, 0:grid_ny - 1, nuvzmax))
  allocate(wwh(0:grid_nx - 1, 0:grid_ny - 1, nwzmax))
  allocate(pvh(0:grid_nx - 1, 0:grid_ny - 1, nuvzmax))
  allocate(rhoh(0:grid_nx - 1, 0:grid_ny - 1, nuvzmax))
  allocate(prsh(0:grid_nx - 1, 0:grid_ny - 1, nuvzmax))
  allocate(pinmconv(0:grid_nx - 1, 0:grid_ny - 1, nzmax))
  allocate(uvzlev(0:grid_nx - 1, 0:grid_ny - 1, nuvzmax))
  uuh = 0.0
  vvh = 0.0
  pvh = 0.0
  do k = 1, native_nz + 1
    level = native_nz + 2 - k
    wwh(:, :, k) = input_omega(level)
  enddo

  call verttransform_init(1)
  do memory = 1, numwfmem
    call verttransform_ecmwf_heights(nxmin1, nymin1, &
      tt2(0:nxmin1, 0:nymin1, 1, memory), &
      td2(0:nxmin1, 0:nymin1, 1, memory), &
      ps(0:nxmin1, 0:nymin1, 1, memory), &
      qvh(0:nxmin1, 0:nymin1, :, memory), &
      tth(0:nxmin1, 0:nymin1, :, memory), prsh, rhoh, pinmconv, &
      uvzlev, etawheight(0:nxmin1, 0:nymin1, :, memory))
    call verttransform_ecmwf_windfields(memory, nxmin1, nymin1, &
      uuh, vvh, wwh, pvh, rhoh, prsh, pinmconv)
  enddo

  open(newunit=output_unit, file=trim(output_path), status="replace", action="write")
  write(output_unit, '(A)') "FLEXPART_W_PRODUCTION_ORACLE_V1"
  write(output_unit, '(A,1X,I0)') "NATIVE_LEVELS", native_nz
  write(output_unit, '(A,1X,I0)') "MODEL_LEVELS", nz
  do k = 1, nz
    write(output_unit, '(I0,2(1X,ES24.16E3))') k, height(k), ww(0, 0, k, 1)
  enddo
  write(output_unit, '(A,1X,I0)') "INTERFACES", nwz
  do k = 1, nwz
    write(output_unit, '(I0,3(1X,ES24.16E3))') k, &
      etawheight(0, 0, k, 1), wwh(0, 0, k) * pinmconv(0, 0, k), wwh(0, 0, k)
  enddo
  write(output_unit, '(A,1X,I0)') "QUERIES", nquery
  do query = 1, nquery
    read(input_unit, *) query_kind, k, fraction
    select case (query_kind)
      case (0)
        particle_height = height(1)
      case (1)
        if (k < 1 .or. k >= nz) error stop "interior bracket is out of range"
        if (fraction <= 0.0 .or. fraction >= 1.0) then
          error stop "interior fraction must be strict"
        endif
        particle_height = height(k) + fraction * (height(k + 1) - height(k))
      case (2)
        particle_height = height(nz)
      case default
        error stop "unknown query kind"
    end select
    call interpol_wind(1800, 0.25, 0.25, particle_height, 0.0)
    write(output_unit, '(I0,1X,I0,1X,I0,3(1X,ES24.16E3))') &
      query, query_kind, k, fraction, particle_height, sampled_w
  enddo
  close(input_unit)
  close(output_unit)

contains

  subroutine configure_flexpart(layer_count)
    integer, intent(in) :: layer_count

    nxmax = grid_nx
    nymax = grid_ny
    nx = grid_nx
    ny = grid_ny
    nxfield = grid_nx
    nxmin1 = grid_nx - 1
    nymin1 = grid_ny - 1
    nuvzmax = layer_count + 1
    nwzmax = layer_count + 1
    nzmax = layer_count + 1
    nuvz = layer_count + 1
    nwz = layer_count + 1
    nz = layer_count + 1
    numbnests = 0
    xglobal = .false.
    sglobal = .false.
    nglobal = .false.
    lcw = .false.
    lcwsum = .false.
    ipin = 0
    loutrestart = -1
    grid_dx = 1.0
    grid_dy = 1.0
    grid_xlon0 = 0.0
    grid_ylat0 = 0.0
    dxconst = 0.0
    dyconst = 0.0
    memtime(1) = 0
    memtime(2) = 3600
    memind(1) = 1
    memind(2) = 2
    memind(3) = 1
  end subroutine configure_flexpart

  subroutine populate_vertical_state(layer_count, surface_pressure_value, &
      surface_temperature, surface_dewpoint, a, b, temperature, humidity)
    integer, intent(in) :: layer_count
    real, intent(in) :: surface_pressure_value, surface_temperature, surface_dewpoint
    real, intent(in) :: a(:), b(:), temperature(:), humidity(:)
    integer :: physical_level, source_level

    akm = 0.0
    bkm = 0.0
    do physical_level = 1, layer_count + 1
      source_level = layer_count + 2 - physical_level
      akm(physical_level) = a(source_level)
      bkm(physical_level) = b(source_level)
    enddo
    akz = 0.0
    bkz = 0.0
    akz(1) = 0.0
    bkz(1) = 1.0
    do physical_level = 1, layer_count
      source_level = layer_count - physical_level + 1
      akz(physical_level + 1) = 0.5 * (a(source_level) + a(source_level + 1))
      bkz(physical_level + 1) = 0.5 * (b(source_level) + b(source_level + 1))
    enddo
    aknew = akz
    bknew = bkz

    tt2 = surface_temperature
    td2 = surface_dewpoint
    ps = surface_pressure_value
    tth = 0.0
    qvh = 0.0
    do physical_level = 1, layer_count
      source_level = layer_count - physical_level + 1
      tth(:, :, physical_level + 1, :) = temperature(source_level)
      qvh(:, :, physical_level + 1, :) = humidity(source_level)
    enddo
  end subroutine populate_vertical_state

end program w_production_oracle
