program interpolation_direct_oracle
  ! ---------------------------------------------------------------------------
  ! Direct execution oracle for the pinned FLEXPART 11.1 interpolation routines
  ! (RISK-03.3G-10c1, issue #71).
  !
  ! The program never re-implements the sampling equations. It initializes only
  ! the pristine module state that the pinned routines consume and then calls
  ! the compiled FLEXPART routines themselves:
  !
  !   geographic: point_mod::coordtrafo, then the horizontal routines below
  !   horizontal: find_grid_indices, find_grid_distances, hor_interpol_4d
  !   vertical:   find_z_level_meters, find_vert_vars, vert_interpol
  !   temporal:   find_time_vars, temporal_interpolation
  !   rain:       interpol_rain
  !
  ! The build linked against the objects of the pinned full FLEXPART build is
  ! authoritative; see docs/interpolation-contract.md.
  ! ---------------------------------------------------------------------------
  use par_mod, only: numwfmem, numpf, icmv
  use com_mod, only: memtime, memind, numbnests, xglobal, sglobal, nglobal, lcw, &
    numpoint, ipin
  use point_mod, only: grid_dx => dx, grid_dy => dy, grid_xlon0 => xlon0, &
    grid_ylat0 => ylat0, xpoint1, xpoint2, ypoint1, ypoint2, coordtrafo
  use windfields_mod, only: nxmax, nymax, nzmax, nx, ny, nz, nxfield, &
    nxmin1, nymin1, height, lsprec, convprec, tcc, tt, ctwc, icloudbot, &
    icloudtop
  use interpol_mod, only: find_grid_indices, find_grid_distances, &
    hor_interpol_4d, find_z_level_meters, find_vert_vars, vert_interpol, &
    find_time_vars, temporal_interpolation, interpol_rain, &
    p1, p2, p3, p4, ix, jy, ixp, jyp, ngrid, indz, indzp, lbounds, &
    dt1, dt2, dtt
  implicit none

  integer, parameter :: ncoord_model = 0
  integer, parameter :: ncoord_interface = 1

  character(len=64) :: mode
  integer :: input_unit, output_unit, i
  character(len=1024) :: input_path, output_path

  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)
  if (len_trim(input_path) == 0 .or. len_trim(output_path) == 0) then
    error stop "usage: interpolation_direct_oracle <input.txt> <output.txt>"
  endif

  ! The pinned build is a mother-domain (non-nested), metdata-free run for the
  ! routines below. Force the state those routines assume.
  numbnests = 0
  xglobal = .false.
  nglobal = .false.
  sglobal = .false.
  ngrid = 0
  memind(1) = 1
  memind(2) = 2
  memind(3) = 1

  open(newunit=input_unit, file=trim(input_path), status="old", action="read")
  open(newunit=output_unit, file=trim(output_path), status="replace", action="write")
  write(output_unit, '(A)') "FLEXPART_INTERPOLATION_ROUTINE_ORACLE_V1"

  read(input_unit, *) mode
  select case (trim(mode))
    case ("horizontal")
      call run_horizontal(input_unit, output_unit, .false.)
    case ("horizontal_geographic")
      call run_horizontal(input_unit, output_unit, .true.)
    case ("vertical")
      call run_vertical(input_unit, output_unit)
    case ("temporal")
      call run_temporal(input_unit, output_unit)
    case ("rain")
      call run_rain(input_unit, output_unit)
    case default
      error stop "unknown oracle mode"
  end select

  close(input_unit)
  close(output_unit)

contains

  subroutine read_reals(unit, values)
    integer, intent(in) :: unit
    real, intent(out) :: values(:)
    integer :: n
    do n = 1, size(values)
      read(unit, *) values(n)
    enddo
  end subroutine read_reals

  subroutine configure_grid(canonical_nx, canonical_ny, periodic, &
      canonical_xlon0, canonical_ylat0, canonical_dx, canonical_dy)
    ! Reproduce the pinned gridcheck_ecmwf mother-domain state for a canonical
    ! cell-center grid. A periodic canonical X grid (nx*dx == 360) corresponds
    ! to FLEXPART's global layout with one extra wrapped column (nx = nxfield+1).
    integer, intent(in) :: canonical_nx, canonical_ny, periodic
    real, intent(in) :: canonical_xlon0, canonical_ylat0
    real, intent(in) :: canonical_dx, canonical_dy

    if (canonical_dx <= 0.0 .or. canonical_dy <= 0.0) then
      error stop "grid spacing must be positive"
    endif
    grid_xlon0 = canonical_xlon0
    grid_ylat0 = canonical_ylat0
    grid_dx = canonical_dx
    grid_dy = canonical_dy

    nxfield = canonical_nx
    ny = canonical_ny
    nymax = canonical_ny
    nymin1 = canonical_ny - 1
    if (periodic == 1) then
      xglobal = .true.
      nxmax = canonical_nx + 1
      nx = canonical_nx + 1
    else
      xglobal = .false.
      nxmax = canonical_nx
      nx = canonical_nx
    endif
    nxmin1 = nx - 1
  end subroutine configure_grid

  subroutine transform_lonlat(lon, lat, xt, yt)
    real, intent(in) :: lon, lat
    real, intent(out) :: xt, yt

    if (allocated(xpoint1)) deallocate(xpoint1)
    if (allocated(xpoint2)) deallocate(xpoint2)
    if (allocated(ypoint1)) deallocate(ypoint1)
    if (allocated(ypoint2)) deallocate(ypoint2)
    allocate(xpoint1(1), xpoint2(1), ypoint1(1), ypoint2(1))

    numpoint = 1
    ipin = 0
    xpoint1(1) = lon
    xpoint2(1) = lon
    ypoint1(1) = lat
    ypoint2(1) = lat
    call coordtrafo(nxmin1, nymin1)
    if (numpoint /= 1) error stop "coordtrafo rejected in-domain oracle query"

    xt = xpoint1(1)
    yt = ypoint1(1)
    deallocate(xpoint1, xpoint2, ypoint1, ypoint2)
  end subroutine transform_lonlat

  subroutine run_horizontal(input_unit, output_unit, geographic)
    integer, intent(in) :: input_unit, output_unit
    logical, intent(in) :: geographic
    integer :: canonical_nx, canonical_ny, canonical_nz, periodic
    integer :: nquery, q
    real :: xlon0, ylat0, dx, dy, xt, yt, lon, lat
    integer :: k
    real, allocatable :: field(:, :, :, :)
    real :: output
    integer :: ixmax, jymax

    read(input_unit, *) canonical_nx, canonical_ny, canonical_nz
    read(input_unit, *) xlon0, ylat0, dx, dy
    read(input_unit, *) periodic
    if (canonical_nx < 1 .or. canonical_ny < 1 .or. canonical_nz < 1) then
      error stop "invalid grid dimensions"
    endif

    call configure_grid(canonical_nx, canonical_ny, periodic, xlon0, ylat0, dx, dy)
    nzmax = canonical_nz
    nz = canonical_nz
    if (allocated(height)) deallocate(height)
    allocate(height(nzmax))

    allocate(field(0:nxmax - 1, 0:nymax - 1, nzmax, numwfmem))
    field = 0.0
    ixmax = canonical_nx - 1
    jymax = canonical_ny - 1
    do k = 1, nz
      do q = 0, jymax
        do i = 0, ixmax
          read(input_unit, *) field(i, q, k, 1)
        enddo
      enddo
      ! The wrapped FLEXPART column duplicates the first canonical column.
      if (periodic == 1) then
        do q = 0, jymax
          field(canonical_nx, q, k, 1) = field(0, q, k, 1)
        enddo
      endif
    enddo
    field(:, :, :, 2) = field(:, :, :, 1)

    read(input_unit, *) nquery
    if (geographic) then
      write(output_unit, '(A)') "MODE horizontal_geographic"
    else
      write(output_unit, '(A)') "MODE horizontal"
    endif
    write(output_unit, '(A,1X,I0)') "CANONICAL_NX", canonical_nx
    write(output_unit, '(A,1X,I0)') "CANONICAL_NY", canonical_ny
    write(output_unit, '(A,1X,I0)') "FLEXPART_NXMAX", nxmax
    write(output_unit, '(A,1X,I0)') "FLEXPART_NYMAX", nymax
    write(output_unit, '(A,1X,I0)') "PERIODIC", periodic
    write(output_unit, '(A,1X,I0)') "NQUERY", nquery
    do q = 1, nquery
      if (geographic) then
        read(input_unit, *) lon, lat, k
        call transform_lonlat(lon, lat, xt, yt)
      else
        read(input_unit, *) xt, yt, k
      endif
      call find_grid_indices(xt, yt)
      call find_grid_distances(xt, yt)
      call hor_interpol_4d(field, output, k, 1, nzmax)
      write(output_unit, '(A)') "QUERY"
      if (geographic) then
        write(output_unit, '(A,1X,ES24.16E3,1X,ES24.16E3)') "LONLAT", lon, lat
      endif
      write(output_unit, '(A,1X,ES24.16E3,1X,ES24.16E3,1X,I0)') "XY", xt, yt, k
      write(output_unit, '(A,4(1X,I0))') "INDICES", ix, jy, ixp, jyp
      write(output_unit, '(A,4(1X,ES24.16E3))') "WEIGHTS", p1, p2, p3, p4
      write(output_unit, '(A,1X,ES24.16E3)') "VALUE", output
    enddo
    deallocate(field)
  end subroutine run_horizontal

  subroutine run_vertical(input_unit, output_unit)
    integer, intent(in) :: input_unit, output_unit
    integer :: nlevel, nquery, q, coordinate, nvals
    real, allocatable :: levels(:), values(:)
    real :: zt, dz1, dz2, output

    read(input_unit, *) nlevel
    if (nlevel < 2) error stop "vertical coordinate needs at least two levels"
    allocate(levels(nlevel))
    call read_reals(input_unit, levels)

    read(input_unit, *) nvals
    if (nvals /= nlevel) error stop "value profile length mismatch"
    allocate(values(nlevel))
    call read_reals(input_unit, values)

    call configure_vertical(nlevel)
    height(1:nlevel) = levels

    read(input_unit, *) nquery
    write(output_unit, '(A)') "MODE vertical"
    write(output_unit, '(A,1X,I0)') "NLEVEL", nlevel
    write(output_unit, '(A,1X,I0)') "NQUERY", nquery
    do q = 1, nquery
      read(input_unit, *) coordinate, zt
      if (coordinate /= ncoord_model .and. coordinate /= ncoord_interface) then
        error stop "vertical coordinate id must be 0 (model) or 1 (interface)"
      endif
      call find_z_level_meters(zt)
      call find_vert_vars(height, zt, indz, dz1, dz2, lbounds, .false.)
      call vert_interpol(values(indz), values(indzp), dz1, dz2, output)
      write(output_unit, '(A)') "QUERY"
      write(output_unit, '(A,1X,I0,1X,ES24.16E3)') "COORD_ZT", coordinate, zt
      write(output_unit, '(A,2(1X,I0))') "LEVELS", indz, indzp
      write(output_unit, '(A,2(1X,L1))') "BOUNDS", lbounds(1), lbounds(2)
      write(output_unit, '(A,2(1X,ES24.16E3))') "DZ", dz1, dz2
      write(output_unit, '(A,1X,ES24.16E3)') "VALUE", output
    enddo
    deallocate(levels, values)
  end subroutine run_vertical

  subroutine configure_vertical(nlevel)
    integer, intent(in) :: nlevel
    nzmax = nlevel
    nz = nlevel
    if (allocated(height)) deallocate(height)
    allocate(height(nzmax))
  end subroutine configure_vertical

  subroutine run_temporal(input_unit, output_unit)
    integer, intent(in) :: input_unit, output_unit
    integer :: nquery, q, itime
    integer :: valid_time_1, valid_time_2
    real :: time1, time2, output

    read(input_unit, *) valid_time_1, valid_time_2
    if (valid_time_2 < valid_time_1) then
      error stop "memtime(2) must not precede memtime(1)"
    endif
    memtime(1) = valid_time_1
    memtime(2) = valid_time_2

    read(input_unit, *) nquery
    write(output_unit, '(A)') "MODE temporal"
    write(output_unit, '(A,2(1X,I0))') "MEMTIME", memtime(1), memtime(2)
    write(output_unit, '(A,1X,I0)') "NQUERY", nquery
    do q = 1, nquery
      read(input_unit, *) itime, time1, time2
      call find_time_vars(itime)
      call temporal_interpolation(time1, time2, output)
      write(output_unit, '(A)') "QUERY"
      write(output_unit, '(A,1X,I0)') "ITIME", itime
      write(output_unit, '(A,2(1X,ES24.16E3))') "INPUTS", time1, time2
      write(output_unit, '(A,3(1X,ES24.16E3))') "DTS", dt1, dt2, dtt
      write(output_unit, '(A,1X,ES24.16E3)') "VALUE", output
    enddo
  end subroutine run_temporal

  subroutine run_rain(input_unit, output_unit)
    integer, intent(in) :: input_unit, output_unit
    integer :: canonical_nx, canonical_ny, periodic
    integer :: nquery, q, kz
    integer :: valid_time_1, valid_time_2, itime
    integer :: intiy1, intiy2
    real :: xlon0, ylat0, dx, dy
    real :: xt_query, yt_query
    real :: yint1, yint2, yint3, ytint, yint4
    real, allocatable :: plane(:)
    integer :: p, r, c

    read(input_unit, *) canonical_nx, canonical_ny
    read(input_unit, *) xlon0, ylat0, dx, dy
    read(input_unit, *) periodic
    read(input_unit, *) valid_time_1, valid_time_2
    memtime(1) = valid_time_1
    memtime(2) = valid_time_2

    call configure_grid(canonical_nx, canonical_ny, periodic, xlon0, ylat0, dx, dy)
    ! interpol_rain needs temperature (tt), cloud cover (tcc) and a W-level
    ! count only to satisfy its array extents; the wet-deposition coupling is
    ! not part of this contract.
    nzmax = 1
    nz = 1
    if (allocated(height)) deallocate(height)
    allocate(height(max(nzmax, 1)))

    call allocate_rain_fields(canonical_nx, canonical_ny, periodic)

    allocate(plane(max(canonical_nx * canonical_ny, 1)))

    ! Large-scale precipitation [mm/h] as stored by windfields_mod for both
    ! wind-field memories.
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_rain(plane, lsprec(:, :, 1, 1, 1), canonical_nx, canonical_ny, periodic)
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_rain(plane, lsprec(:, :, 1, 1, 2), canonical_nx, canonical_ny, periodic)
    ! Convective precipitation [mm/h].
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_rain(plane, convprec(:, :, 1, 1, 1), canonical_nx, canonical_ny, periodic)
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_rain(plane, convprec(:, :, 1, 1, 2), canonical_nx, canonical_ny, periodic)
    ! Total cloud cover [fraction].
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_plane(plane, tcc(:, :, 1, 1), canonical_nx, canonical_ny, periodic)
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_plane(plane, tcc(:, :, 1, 2), canonical_nx, canonical_ny, periodic)
    ! Particle-level temperature [K] and cloud total water [kg/kg].
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_plane(plane, tt(:, :, 1, 1), canonical_nx, canonical_ny, periodic)
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_plane(plane, tt(:, :, 1, 2), canonical_nx, canonical_ny, periodic)
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_plane(plane, ctwc(:, :, 1), canonical_nx, canonical_ny, periodic)
    call read_plane(input_unit, plane, canonical_nx, canonical_ny)
    call scatter_plane(plane, ctwc(:, :, 2), canonical_nx, canonical_ny, periodic)
    lcw = .true.

    do p = 0, nxmax - 1
      do r = 0, nymax - 1
        icloudbot(p, r, 1) = icmv
        icloudbot(p, r, 2) = icmv
        icloudtop(p, r, 1) = icmv
        icloudtop(p, r, 2) = icmv
      enddo
    enddo

    read(input_unit, *) nquery
    write(output_unit, '(A)') "MODE rain"
    write(output_unit, '(A,1X,I0)') "NUMPF", numpf
    write(output_unit, '(A,1X,I0)') "FLEXPART_NXMAX", nxmax
    write(output_unit, '(A,1X,I0)') "FLEXPART_NYMAX", nymax
    write(output_unit, '(A,1X,I0)') "NQUERY", nquery
    do q = 1, nquery
      read(input_unit, *) xt_query, yt_query, itime, kz
      call find_grid_indices(xt_query, yt_query)
      call find_grid_distances(xt_query, yt_query)
      call find_time_vars(itime)
      call interpol_rain(itime, kz, yint1, yint2, yint3, ytint, yint4, &
        intiy1, intiy2, icmv)
      write(output_unit, '(A)') "QUERY"
      write(output_unit, '(A,1X,ES24.16E3,1X,ES24.16E3,1X,I0,1X,I0)') &
        "XY_ITIME_KZ", xt_query, yt_query, itime, kz
      write(output_unit, '(A,3(1X,ES24.16E3))') "DTS", dt1, dt2, dtt
      write(output_unit, '(A,1X,ES24.16E3)') "LSP", yint1
      write(output_unit, '(A,1X,ES24.16E3)') "CP", yint2
      write(output_unit, '(A,1X,ES24.16E3)') "TCC", yint3
      write(output_unit, '(A,1X,ES24.16E3)') "TT", ytint
      write(output_unit, '(A,1X,ES24.16E3)') "CTWC", yint4
      write(output_unit, '(A,2(1X,I0))') "CLOUD", intiy1, intiy2
    enddo
    deallocate(plane)
  end subroutine run_rain

  subroutine allocate_rain_fields(canonical_nx, canonical_ny, periodic)
    integer, intent(in) :: canonical_nx, canonical_ny, periodic
    if (allocated(lsprec)) deallocate(lsprec)
    if (allocated(convprec)) deallocate(convprec)
    if (allocated(tcc)) deallocate(tcc)
    if (allocated(tt)) deallocate(tt)
    if (allocated(ctwc)) deallocate(ctwc)
    if (allocated(icloudbot)) deallocate(icloudbot)
    if (allocated(icloudtop)) deallocate(icloudtop)
    allocate(lsprec(0:nxmax - 1, 0:nymax - 1, 1, numpf, numwfmem))
    allocate(convprec(0:nxmax - 1, 0:nymax - 1, 1, numpf, numwfmem))
    allocate(tcc(0:nxmax - 1, 0:nymax - 1, 1, numwfmem))
    allocate(tt(0:nxmax - 1, 0:nymax - 1, nzmax, numwfmem))
    allocate(ctwc(0:nxmax - 1, 0:nymax - 1, numwfmem))
    allocate(icloudbot(0:nxmax - 1, 0:nymax - 1, numwfmem))
    allocate(icloudtop(0:nxmax - 1, 0:nymax - 1, numwfmem))
    lsprec = 0.0
    convprec = 0.0
    tcc = 0.0
    tt = 0.0
    ctwc = 0.0
    ! Keep the canonical x_fastest, y-then-x read order explicit even though the
    ! periodic flag is unused here beyond documenting the layout.
    if (periodic /= 0 .and. periodic /= 1) error stop "periodic flag must be 0 or 1"
  end subroutine allocate_rain_fields

  subroutine read_plane(unit, plane, canonical_nx, canonical_ny)
    integer, intent(in) :: unit, canonical_nx, canonical_ny
    real, intent(out) :: plane(:)
    integer :: r, c
    do r = 0, canonical_ny - 1
      do c = 0, canonical_nx - 1
        read(unit, *) plane(c + r * canonical_nx + 1)
      enddo
    enddo
  end subroutine read_plane

  subroutine scatter_plane(plane, target, canonical_nx, canonical_ny, periodic)
    real, intent(in) :: plane(:)
    real, intent(out) :: target(0:, 0:)
    integer, intent(in) :: canonical_nx, canonical_ny, periodic
    integer :: r, c
    do r = 0, canonical_ny - 1
      do c = 0, canonical_nx - 1
        target(c, r) = plane(c + r * canonical_nx + 1)
      enddo
      if (periodic == 1) target(canonical_nx, r) = plane(0 + r * canonical_nx + 1)
    enddo
  end subroutine scatter_plane

  subroutine scatter_rain(plane, target, canonical_nx, canonical_ny, periodic)
    real, intent(in) :: plane(:)
    real, intent(out) :: target(0:, 0:)
    integer, intent(in) :: canonical_nx, canonical_ny, periodic
    call scatter_plane(plane, target, canonical_nx, canonical_ny, periodic)
  end subroutine scatter_rain

end program interpolation_direct_oracle
