program vertical_column_oracle
  use par_mod, only: r_air, ga
  use qvsat_mod, only: ew
  implicit none

  integer :: nz, k, step, j, physical_k, level
  integer :: input_unit, output_unit, has_motion
  character(len=1024) :: input_path, output_path
  real :: ps, t2, td2, terrain
  real :: tvold, tv, pold, pint, const, layer
  real :: dz, dp
  real, allocatable :: a(:), b(:), temperature(:), humidity(:)
  real, allocatable :: pressure(:), height_agl(:)
  real, allocatable :: omega(:), w_ms(:), pinmconv(:), w_height(:)
  real, allocatable :: center_height(:), center_pressure(:)

  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)
  if (len_trim(input_path) == 0 .or. len_trim(output_path) == 0) then
    error stop "usage: vertical_column_oracle <input.txt> <output.txt>"
  endif

  open(newunit=input_unit, file=trim(input_path), status="old", action="read")
  read(input_unit, *) nz
  if (nz < 1) error stop "nz must be positive"

  allocate(a(nz+1), b(nz+1), temperature(nz), humidity(nz))
  allocate(pressure(nz), height_agl(nz))
  allocate(omega(nz+1), w_ms(nz+1), pinmconv(nz+1), w_height(nz+1))
  allocate(center_height(nz+1), center_pressure(nz+1))
  omega=0.0
  w_ms=0.0
  pinmconv=0.0
  w_height=0.0

  read(input_unit, *) ps, t2, td2, terrain
  if (ps <= 0.0 .or. t2 <= 0.0 .or. td2 <= 0.0) then
    error stop "invalid surface state"
  endif

  do k=1,nz+1
    read(input_unit, *) a(k), b(k)
  enddo
  do k=1,nz
    read(input_unit, *) temperature(k), humidity(k)
  enddo

  read(input_unit, *) has_motion
  if (has_motion /= 0 .and. has_motion /= 1) error stop "invalid motion flag"
  if (has_motion == 1) then
    do k=1,nz+1
      read(input_unit, *) omega(k)
    enddo
  endif
  close(input_unit)

  ! Canonical fixture is top->bottom (pressure increasing with k). FLEXPART
  ! adds an artificial ground level with akz=0, bkz=1 and then integrates
  ! bottom->top through the real model levels.
  do k=1,nz
    pressure(k)=0.5*(a(k)+a(k+1)) + 0.5*(b(k)+b(k+1))*ps
    if (pressure(k) <= 0.0) error stop "invalid model-level pressure"
  enddo

  const=r_air/ga
  tvold=t2*(1.0+0.378*ew(td2,ps)/ps)
  pold=ps
  layer=0.0

  do step=1,nz
    k=nz-step+1
    pint=pressure(k)
    if (pint >= pold) error stop "pressure must decrease upward"
    tv=temperature(k)*(1.0+0.608*humidity(k))

    if (abs(tv-tvold).gt.0.2) then
      layer=layer+const*log(pold/pint)*(tv-tvold)/log(tv/tvold)
    else
      layer=layer+const*log(pold/pint)*tv
    endif

    height_agl(k)=layer
    tvold=tv
    pold=pint
  enddo

  ! FLEXPART verttransform_ecmwf_heights W-level geometry.
  center_height(1)=0.0
  center_pressure(1)=ps
  do k=2,nz+1
    level=nz-k+2
    center_height(k)=height_agl(level)
    center_pressure(k)=pressure(level)
  enddo

  w_height(1)=0.0
  if (nz == 1) then
    w_height(2)=center_height(2)
  else
    do k=2,nz
      w_height(k)=0.5*(center_height(k+1)+center_height(k))
    enddo
    w_height(nz+1)=w_height(nz)+center_height(nz+1)-center_height(nz)
  endif

  if (has_motion == 1) then
    ! FLEXPART's pinmconv is dz/dp on the W grid. Reconstruct its physical
    ! bottom->top center coordinate including the artificial surface level.
    if (nz == 1) then
      dz=center_height(2)-center_height(1)
      dp=center_pressure(2)-center_pressure(1)
      if (dp == 0.0) error stop "zero pressure derivative"
      pinmconv(1)=dz/dp
      pinmconv(2)=pinmconv(1)
    else
      dz=center_height(2)-center_height(1)
      dp=center_pressure(2)-center_pressure(1)
      if (dp == 0.0) error stop "zero lower pressure derivative"
      pinmconv(1)=dz/dp

      do k=2,nz
        dz=center_height(k+1)-center_height(k-1)
        dp=center_pressure(k+1)-center_pressure(k-1)
        if (dp == 0.0) error stop "zero centered pressure derivative"
        pinmconv(k)=dz/dp
      enddo

      dz=center_height(nz+1)-center_height(nz)
      dp=center_pressure(nz+1)-center_pressure(nz)
      if (dp == 0.0) error stop "zero upper pressure derivative"
      pinmconv(nz+1)=dz/dp
    endif

    do j=1,nz+1
      ! Canonical interface index j is top->surface; physical pinmconv is
      ! bottom->top, so reverse only for the multiplication.
      physical_k=nz+2-j
      w_ms(j)=omega(j)*pinmconv(physical_k)
    enddo
  endif

  open(newunit=output_unit, file=trim(output_path), status="replace", action="write")
  write(output_unit,'(A)') "FLEXPART_VERTICAL_CONFORMANCE_HARNESS_V1"
  write(output_unit,'(I0)') nz
  do k=1,nz
    write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3,1X,ES24.16E3)') &
      k-1, pressure(k), height_agl(k), height_agl(k)+terrain
  enddo

  write(output_unit,'(A,1X,I0)') "INTERFACES", nz+1
  do j=1,nz+1
    physical_k=nz+2-j
    write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3)') &
      j-1, w_height(physical_k), w_height(physical_k)+terrain
  enddo

  write(output_unit,'(A,1X,I0)') "MOTION", has_motion
  if (has_motion == 1) then
    write(output_unit,'(I0)') nz+1
    do j=1,nz+1
      write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3)') &
        j-1, omega(j), w_ms(j)
    enddo
  endif

  close(output_unit)
end program vertical_column_oracle
