program flexpart_vertical_driver
  use vertical_oracle_state_mod
  use flexpart_vertical_oracle_routine_mod, only: verttransform_ecmwf_heights
  implicit none

  integer :: nreal, k, j, iface, flex_index
  integer :: input_unit, output_unit, has_motion
  character(len=1024) :: input_path, output_path
  real :: ps, t2_surface, td2_surface, terrain

  real, allocatable :: a(:), b(:)
  real, allocatable :: temperature(:), humidity(:), omega(:)
  real, allocatable :: tt2_grid(:,:), td2_grid(:,:), ps_grid(:,:)
  real, allocatable :: qvh(:,:,:), tth(:,:,:)
  real, allocatable :: prsh(:,:,:), rhoh(:,:,:), pinmconv(:,:,:)
  real, allocatable :: uvzlev(:,:,:), wzlev(:,:,:)

  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)
  if (len_trim(input_path) == 0 .or. len_trim(output_path) == 0) then
    error stop "usage: flexpart_vertical_driver <input.txt> <output.txt>"
  endif

  open(newunit=input_unit, file=trim(input_path), status="old", action="read")
  read(input_unit, *) nreal
  if (nreal < 1) error stop "nreal must be positive"

  read(input_unit, *) ps, t2_surface, td2_surface, terrain
  if (ps <= 0.0 .or. t2_surface <= 0.0 .or. td2_surface <= 0.0) then
    error stop "invalid surface state"
  endif

  allocate(a(nreal+1), b(nreal+1))
  allocate(temperature(nreal), humidity(nreal), omega(nreal+1))

  do k=1,nreal+1
    read(input_unit, *) a(k), b(k)
  enddo
  do k=1,nreal
    read(input_unit, *) temperature(k), humidity(k)
  enddo

  read(input_unit, *) has_motion
  if (has_motion /= 0 .and. has_motion /= 1) then
    error stop "invalid motion flag"
  endif
  omega=0.0
  if (has_motion == 1) then
    do k=1,nreal+1
      read(input_unit, *) omega(k)
    enddo
  endif
  close(input_unit)

  ! FLEXPART stores an artificial ground model level at index 1 followed by
  ! real model levels in physical bottom->top order. The canonical fixture is
  ! top->bottom, so build the exact state consumed by verttransform_ecmwf_heights.
  nuvzmax=nreal+1
  nzmax=nreal+1
  nuvz=nreal+1
  nwz=nreal+1
  nz=nreal+1

  allocate(akz(nuvzmax), bkz(nuvzmax), aknew(nzmax), bknew(nzmax))
  akz(1)=0.0
  bkz(1)=1.0
  do k=1,nreal
    j=nreal-k+1
    akz(k+1)=0.5*(a(j)+a(j+1))
    bkz(k+1)=0.5*(b(j)+b(j+1))
  enddo
  aknew=akz
  bknew=bkz

  allocate(tt2_grid(0:0,0:0), td2_grid(0:0,0:0), ps_grid(0:0,0:0))
  allocate(qvh(0:0,0:0,nuvzmax), tth(0:0,0:0,nuvzmax))
  allocate(prsh(0:0,0:0,nuvzmax), rhoh(0:0,0:0,nuvzmax))
  allocate(pinmconv(0:0,0:0,nzmax))
  allocate(uvzlev(0:0,0:0,nuvzmax), wzlev(0:0,0:0,nuvzmax))

  tt2_grid(0,0)=t2_surface
  td2_grid(0,0)=td2_surface
  ps_grid(0,0)=ps
  qvh=0.0
  tth=0.0
  tth(0,0,1)=t2_surface

  do k=1,nreal
    j=nreal-k+1
    tth(0,0,k+1)=temperature(j)
    qvh(0,0,k+1)=humidity(j)
  enddo

  call verttransform_ecmwf_heights(0,0,tt2_grid,td2_grid,ps_grid, &
    qvh,tth,prsh,rhoh,pinmconv,uvzlev,wzlev)

  open(newunit=output_unit, file=trim(output_path), status="replace", action="write")
  write(output_unit,'(A)') "FLEXPART_VERTICAL_ROUTINE_ORACLE_V1"
  write(output_unit,'(I0)') nreal

  do j=1,nreal
    flex_index=nuvz-j+1
    write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3,1X,ES24.16E3)') &
      j-1, prsh(0,0,flex_index), uvzlev(0,0,flex_index), &
      uvzlev(0,0,flex_index)+terrain
  enddo

  write(output_unit,'(A,1X,I0)') "INTERFACES", nreal+1
  do iface=1,nreal+1
    flex_index=nwz-iface+1
    write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3)') &
      iface-1, wzlev(0,0,flex_index), wzlev(0,0,flex_index)+terrain
  enddo

  write(output_unit,'(A,1X,I0)') "MOTION", has_motion
  if (has_motion == 1) then
    write(output_unit,'(I0)') nreal+1
    do iface=1,nreal+1
      flex_index=nz-iface+1
      write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3)') &
        iface-1, omega(iface), omega(iface)*pinmconv(0,0,flex_index)
    enddo
  endif

  close(output_unit)
end program flexpart_vertical_driver
