program vertical_column_oracle
  use par_mod, only: r_air, ga
  use qvsat_mod, only: ew
  implicit none

  integer :: nz, k, step, input_unit, output_unit
  character(len=1024) :: input_path, output_path
  real :: ps, t2, td2, terrain
  real :: tvold, tv, pold, pint, const, layer
  real, allocatable :: a(:), b(:), temperature(:), humidity(:)
  real, allocatable :: pressure(:), height_agl(:)

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

  open(newunit=output_unit, file=trim(output_path), status="replace", action="write")
  write(output_unit,'(A)') "FLEXPART_VERTICAL_COLUMN_ORACLE_V1"
  write(output_unit,'(I0)') nz
  do k=1,nz
    write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3,1X,ES24.16E3)') &
      k-1, pressure(k), height_agl(k), height_agl(k)+terrain
  enddo
  close(output_unit)
end program vertical_column_oracle
