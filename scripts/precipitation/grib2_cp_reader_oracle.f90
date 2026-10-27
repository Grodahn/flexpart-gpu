program grib2_cp_reader_oracle
  ! Direct pinned FLEXPART 11.1 reader oracle for #211 (GRIB2 CP units).
  !
  ! Runs the unchanged gridcheck_ecmwf and readwind_ecmwf on one complete
  ! prepared input file and reports the stored precipitation memory slots
  ! lsprec(:,:,1,ipf,n) and convprec(:,:,1,ipf,n) [declared mm/h,
  ! windfields_mod.f90:157-158]. For GRIB2 the CP branch divides by
  ! conversion_factor=1000 (windfields_mod.f90:2145-2147, 2344-2349); LSP is
  ! stored unscaled (2143-2144, 2336-2342). No upstream routine is copied.
  use par_mod, only: numpf
  use com_mod, only: path, length, ideltas, nxshift, numbnests, nxmaxn, nymaxn, lprecint
  use windfields_mod, only: gridcheck_ecmwf, readwind_ecmwf, wfname, nxmax, nymax, &
    nuvzmax, nwzmax, nx, ny, nxfield, nlev_ec, nuvz, nwz, lsprec, convprec
  implicit none
  integer, parameter :: memory_slot = 1, file_index = 1
  integer :: output_unit, i, j, ipf
  character(len=1024) :: input_dir, input_name, output_path
  real(kind=4), allocatable :: uuh(:,:,:), vvh(:,:,:), wwh(:,:,:)

  call get_command_argument(1, input_dir)
  call get_command_argument(2, input_name)
  call get_command_argument(3, output_path)
  if (len_trim(input_dir) == 0 .or. len_trim(input_name) == 0 .or. len_trim(output_path) == 0) &
    error stop 'usage: grib2-cp-reader-oracle <input-dir/> <file> <output>'

  ! Forward run state read by gridcheck_ecmwf (ifn=1 when ideltas>0) and the
  ! regional-domain requirement nxshift=0 (windfields_mod.f90:395-399, 655-657).
  ideltas=1
  nxshift=0
  numbnests=0
  nxmaxn=0
  nymaxn=0
  path(3)=trim(input_dir)
  length(3)=len_trim(input_dir)
  allocate(wfname(1))
  wfname(file_index)=trim(input_name)

  call gridcheck_ecmwf
  allocate(uuh(0:nxmax-1,0:nymax-1,nuvzmax), vvh(0:nxmax-1,0:nymax-1,nuvzmax))
  allocate(wwh(0:nxmax-1,0:nymax-1,nwzmax))
  call readwind_ecmwf(file_index, memory_slot, uuh, vvh, wwh)

  open(newunit=output_unit, file=trim(output_path), status='replace', action='write')
  write(output_unit,'(A)') 'FLEXPART_GRIB2_CP_READER_ORACLE_V1'
  write(output_unit,'(A,1X,A)') 'FILE', trim(input_name)
  write(output_unit,'(A,6(1X,I0),1X,L1)') 'GRID', nx, ny, nxfield, nlev_ec, nuvz, nwz, lprecint
  write(output_unit,'(A,2(1X,I0))') 'NUMPF', numpf, memory_slot
  ! One row per FLEXPART node (i, j), j=0 south; values in the reader's real4.
  do ipf=1,numpf
    do j=0,ny-1
      do i=0,nxfield-1
        write(output_unit,'(A,3(1X,I0),2(1X,ES16.9E2))') 'NODE', ipf, i, j, &
          lsprec(i,j,1,ipf,memory_slot), convprec(i,j,1,ipf,memory_slot)
      enddo
    enddo
  enddo
  close(output_unit)
end program grib2_cp_reader_oracle
