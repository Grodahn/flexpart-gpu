program shared_height_oracle
  ! Research-only state plumbing for pinned eta=no production routines (#118).
  ! All heights, field remapping and wind samples are computed by upstream code.
  use com_mod, only: memtime, memind, numbnests, xglobal, sglobal, nglobal, &
    lcw, lcwsum, ipin, loutrestart
  use point_mod, only: grid_dx => dx, grid_dy => dy, &
    grid_xlon0 => xlon0, grid_ylat0 => ylat0
  use windfields_mod, only: nxmax, nymax, nuvzmax, nwzmax, nzmax, nx, ny, &
    nz, nxfield, nxmin1, nymin1, nuvz, nwz, dxconst, dyconst, height, &
    akm, bkm, akz, bkz, aknew, bknew, tt2, td2, ps, tth, qvh, &
    etauvheight, etawheight, uu, vv, ww, oro, alloc_windfields, alloc_fixedfields
  use verttransform_mod, only: verttransform_init, &
    verttransform_ecmwf_heights, verttransform_ecmwf_windfields
  use interpol_mod, only: interpol_wind, sampled_u => u, sampled_v => v, sampled_w => w
  implicit none
  integer :: input_unit, output_unit, layers, k, x, memory, query, nquery, itime
  real :: query_x, query_y, fraction, z, terrain_query, surface_p, surface_t, surface_td
  real, allocatable :: a(:), b(:), uuh(:,:,:), vvh(:,:,:), wwh(:,:,:), pvh(:,:,:)
  real, allocatable :: rhoh(:,:,:), prsh(:,:,:), pinmconv(:,:,:)
  character(len=1024) :: input_path, output_path

  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)
  open(newunit=input_unit, file=trim(input_path), status='old', action='read')
  read(input_unit,*) layers
  if (layers < 2) error stop 'at least two layers required'
  nxmax=2; nymax=2; nx=2; ny=2; nxfield=2; nxmin1=1; nymin1=1
  nuvzmax=layers+1; nwzmax=layers+1; nzmax=layers+1
  nuvz=layers+1; nwz=layers+1; nz=layers+1
  numbnests=0; xglobal=.false.; sglobal=.false.; nglobal=.false.
  lcw=.false.; lcwsum=.false.; ipin=0; loutrestart=-1
  grid_dx=1.; grid_dy=1.; grid_xlon0=0.; grid_ylat0=0.
  ! Physical inverse-degree distances; the regional 2x2 boundary excludes slopes.
  dxconst=1./111200.; dyconst=1./111200.
  memtime(1)=0; memtime(2)=3600; memind(1)=1; memind(2)=2; memind(3)=1
  call alloc_fixedfields
  call alloc_windfields
  allocate(a(layers+1), b(layers+1))
  ! Explicit bottom-to-top half-level coefficients, surface first.
  do k=1,layers+1
    read(input_unit,*) a(k), b(k)
  enddo
  akm=a; bkm=b; akz=0.; bkz=0.; bkz(1)=1.
  do k=2,layers+1
    akz(k)=0.5*(a(k-1)+a(k)); bkz(k)=0.5*(b(k-1)+b(k))
  enddo
  aknew=akz; bknew=bkz
  read(input_unit,*) oro(0,0), oro(1,0)
  oro(:,1)=oro(:,0)
  allocate(uuh(0:1,0:1,nuvzmax), vvh(0:1,0:1,nuvzmax))
  allocate(wwh(0:1,0:1,nwzmax), pvh(0:1,0:1,nuvzmax))
  allocate(rhoh(0:1,0:1,nuvzmax), prsh(0:1,0:1,nuvzmax), pinmconv(0:1,0:1,nzmax))
  open(newunit=output_unit, file=trim(output_path), status='replace', action='write')
  write(output_unit,'(A)') 'FLEXPART_SHARED_HEIGHT_ORACLE_V1'
  do memory=1,2
    do x=0,1
      read(input_unit,*) surface_p, surface_t, surface_td
      ps(x,:,1,memory)=surface_p; tt2(x,:,1,memory)=surface_t; td2(x,:,1,memory)=surface_td
      do k=1,nz
        read(input_unit,*) tth(x,0,k,memory), qvh(x,0,k,memory), &
          uuh(x,0,k), vvh(x,0,k), wwh(x,0,k)
        tth(x,1,k,memory)=tth(x,0,k,memory); qvh(x,1,k,memory)=qvh(x,0,k,memory)
        uuh(x,1,k)=uuh(x,0,k); vvh(x,1,k)=vvh(x,0,k); wwh(x,1,k)=wwh(x,0,k)
      enddo
    enddo
    pvh=0.
    if (memory == 1) call verttransform_init(memory)
    call verttransform_ecmwf_heights(nxmin1, nymin1, tt2(:,:,1,memory), &
      td2(:,:,1,memory), ps(:,:,1,memory), qvh(:,:,:,memory), tth(:,:,:,memory), &
      prsh, rhoh, pinmconv, etauvheight(:,:,:,memory), etawheight(:,:,:,memory))
    call verttransform_ecmwf_windfields(memory, nxmin1, nymin1, uuh, vvh, wwh, pvh, rhoh, prsh, pinmconv)
    do x=0,1
      do k=1,nz
        write(output_unit,'(A,3(1X,I0),8(1X,ES24.16E3))') 'NATIVE', memory, x, k, &
          etauvheight(x,0,k,memory), etawheight(x,0,k,memory), prsh(x,0,k), &
          pinmconv(x,0,k), uuh(x,0,k), vvh(x,0,k), wwh(x,0,k), wwh(x,0,k)*pinmconv(x,0,k)
        write(output_unit,'(A,3(1X,I0),4(1X,ES24.16E3))') 'SHARED', memory, x, k, &
          height(k), uu(x,0,k,memory), vv(x,0,k,memory), ww(x,0,k,memory)
      enddo
    enddo
  enddo
  read(input_unit,*) nquery
  do query=1,nquery
    ! k=0 selects lower boundary, k=nz selects upper boundary.
    read(input_unit,*) itime, query_x, query_y, k, fraction
    if (k==0) then
      z=height(1)
    else if (k==nz) then
      z=height(nz)
    else
      if (k<1 .or. k>=nz .or. fraction<=0. .or. fraction>=1.) error stop 'invalid interior query'
      z=height(k)+fraction*(height(k+1)-height(k))
    endif
    ! interpol_wind accepts AGL only. ASL here is an explicitly labelled
    ! diagnostic equivalent, using repeated-y bilinear terrain, not a new oracle API.
    terrain_query=(1.-query_x)*oro(0,0)+query_x*oro(1,0)
    call interpol_wind(itime, query_x, query_y, z, 0.)
    write(output_unit,'(A,2(1X,I0),7(1X,ES24.16E3))') 'QUERY', query, itime, &
      query_x, query_y, z, z+terrain_query, sampled_u, sampled_v, sampled_w
  enddo
  close(input_unit)
  close(output_unit)
end program shared_height_oracle
