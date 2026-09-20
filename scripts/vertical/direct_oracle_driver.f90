program vertical_direct_oracle
  use verttransform_mod, only: verttransform_ecmwf_heights
  use windfields_mod, only: nuvz, nwz, nz, nuvzmax, nwzmax, nzmax, &
    akz, bkz, aknew, bknew
  implicit none

  integer :: native_nz, k, j, level, physical_k
  integer :: input_unit, output_unit, has_motion
  character(len=1024) :: input_path, output_path
  real :: ps, t2, td2, terrain
  real, allocatable :: a(:), b(:), temperature(:), humidity(:)
  real, allocatable :: omega(:), w_ms(:)
  real, allocatable :: tt2_tmp(:,:), td2_tmp(:,:), ps_tmp(:,:)
  real, allocatable :: qvh_tmp(:,:,:), tth_tmp(:,:,:)
  real, allocatable :: prsh_tmp(:,:,:), rhoh_tmp(:,:,:)
  real, allocatable :: pinmconv(:,:,:), uvzlev(:,:,:), wzlev(:,:,:)

  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)
  if (len_trim(input_path) == 0 .or. len_trim(output_path) == 0) then
    error stop "usage: vertical_direct_oracle <input.txt> <output.txt>"
  endif

  open(newunit=input_unit, file=trim(input_path), status="old", action="read")
  read(input_unit, *) native_nz
  if (native_nz < 1) error stop "native_nz must be positive"

  allocate(a(native_nz+1), b(native_nz+1))
  allocate(temperature(native_nz), humidity(native_nz))
  allocate(omega(native_nz+1), w_ms(native_nz+1))
  omega = 0.0
  w_ms = 0.0

  read(input_unit, *) ps, t2, td2, terrain
  if (ps <= 0.0 .or. t2 <= 0.0 .or. td2 <= 0.0) then
    error stop "invalid surface state"
  endif

  do k=1,native_nz+1
    read(input_unit, *) a(k), b(k)
  enddo
  do k=1,native_nz
    read(input_unit, *) temperature(k), humidity(k)
  enddo

  read(input_unit, *) has_motion
  if (has_motion /= 0 .and. has_motion /= 1) error stop "invalid motion flag"
  if (has_motion == 1) then
    do k=1,native_nz+1
      read(input_unit, *) omega(k)
    enddo
  endif
  close(input_unit)

  ! Reproduce only the windfields_mod state that gridcheck_ecmwf establishes
  ! before the real FLEXPART routine is called. The calculation itself below
  ! is performed by the pristine verttransform_mod implementation.
  nuvzmax = native_nz + 1
  nwzmax = native_nz + 1
  nzmax = native_nz + 1
  nuvz = native_nz + 1
  nwz = native_nz + 1
  nz = native_nz + 1

  if (allocated(akz)) deallocate(akz)
  if (allocated(bkz)) deallocate(bkz)
  if (allocated(aknew)) deallocate(aknew)
  if (allocated(bknew)) deallocate(bknew)
  allocate(akz(nuvzmax), bkz(nuvzmax), aknew(nzmax), bknew(nzmax))

  ! Canonical input is top -> surface. FLEXPART stores the artificial surface
  ! level first and then real model-level centers bottom -> top.
  akz = 0.0
  bkz = 0.0
  aknew = 0.0
  bknew = 0.0
  akz(1) = 0.0
  bkz(1) = 1.0
  do k=1,native_nz
    level = native_nz - k + 1
    akz(k+1) = 0.5 * (a(level) + a(level+1))
    bkz(k+1) = 0.5 * (b(level) + b(level+1))
  enddo
  aknew(1:nz) = akz(1:nz)
  bknew(1:nz) = bkz(1:nz)

  allocate(tt2_tmp(0:0,0:0), td2_tmp(0:0,0:0), ps_tmp(0:0,0:0))
  allocate(qvh_tmp(0:0,0:0,nuvzmax), tth_tmp(0:0,0:0,nuvzmax))
  allocate(prsh_tmp(0:0,0:0,nuvzmax), rhoh_tmp(0:0,0:0,nuvzmax))
  allocate(pinmconv(0:0,0:0,nzmax))
  allocate(uvzlev(0:0,0:0,nuvzmax), wzlev(0:0,0:0,nuvzmax))

  tt2_tmp(0,0) = t2
  td2_tmp(0,0) = td2
  ps_tmp(0,0) = ps
  qvh_tmp = 0.0
  tth_tmp = 0.0
  do k=1,native_nz
    level = native_nz - k + 1
    tth_tmp(0,0,k+1) = temperature(level)
    qvh_tmp(0,0,k+1) = humidity(level)
  enddo

  call verttransform_ecmwf_heights(0, 0, tt2_tmp, td2_tmp, ps_tmp, &
    qvh_tmp, tth_tmp, prsh_tmp, rhoh_tmp, pinmconv, uvzlev, wzlev)

  if (has_motion == 1) then
    do j=1,native_nz+1
      physical_k = native_nz + 2 - j
      w_ms(j) = omega(j) * pinmconv(0,0,physical_k)
    enddo
  endif

  open(newunit=output_unit, file=trim(output_path), status="replace", action="write")
  write(output_unit,'(A)') "FLEXPART_VERTICAL_ROUTINE_ORACLE_V1"
  write(output_unit,'(I0)') native_nz

  do j=1,native_nz
    physical_k = native_nz - j + 2
    write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3,1X,ES24.16E3)') &
      j-1, prsh_tmp(0,0,physical_k), uvzlev(0,0,physical_k), &
      uvzlev(0,0,physical_k) + terrain
  enddo

  write(output_unit,'(A,1X,I0)') "INTERFACES", native_nz+1
  do j=1,native_nz+1
    physical_k = native_nz + 2 - j
    write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3)') &
      j-1, wzlev(0,0,physical_k), wzlev(0,0,physical_k) + terrain
  enddo

  write(output_unit,'(A,1X,I0)') "MOTION", has_motion
  if (has_motion == 1) then
    write(output_unit,'(I0)') native_nz+1
    do j=1,native_nz+1
      write(output_unit,'(I0,1X,ES24.16E3,1X,ES24.16E3)') &
        j-1, omega(j), w_ms(j)
    enddo
  endif
  close(output_unit)
end program vertical_direct_oracle
