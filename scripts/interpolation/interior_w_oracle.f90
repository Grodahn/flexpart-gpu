program interior_w_oracle
  ! Research-only state plumbing for pinned eta=no production routines (#186).
  ! All heights, field remapping and wind samples are computed by upstream code.
  use par_mod, only: pi, r_earth
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
  integer :: input_unit, output_unit, layers, k, x, y, memory, query, nquery, itime, variant, ix, iy
  real :: physical_dxconst, physical_dyconst
  character(len=4), parameter :: labels(4)=[character(len=4) :: "full", "x", "y", "zero"]
  real :: query_x, query_y, fraction, z, terrain_query, surface_p, surface_t, surface_td
  real, allocatable :: a(:), b(:), uuh(:,:,:), vvh(:,:,:), wwh(:,:,:), pvh(:,:,:)
  real, allocatable :: saved_u(:,:,:,:,:), saved_v(:,:,:,:,:), saved_w(:,:,:,:,:)
  real, allocatable :: rhoh(:,:,:), prsh(:,:,:), pinmconv(:,:,:)
  character(len=1024) :: input_path, output_path

  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)
  open(newunit=input_unit, file=trim(input_path), status='old', action='read')
  read(input_unit,*) layers
  if (layers < 2) error stop 'at least two layers required'
  nxmax=3; nymax=3; nx=3; ny=3; nxfield=3; nxmin1=2; nymin1=2
  nuvzmax=layers+1; nwzmax=layers+1; nzmax=layers+1
  nuvz=layers+1; nwz=layers+1; nz=layers+1
  numbnests=0; xglobal=.false.; sglobal=.false.; nglobal=.false.
  lcw=.false.; lcwsum=.false.; ipin=0; loutrestart=-1
  read(input_unit,*) grid_dx, grid_dy, grid_xlon0, grid_ylat0
  ! Pinned gridcheck_ecmwf spacing, windfields_mod.f90:630-631.
  physical_dxconst=180./(grid_dx*r_earth*pi)
  physical_dyconst=180./(grid_dy*r_earth*pi)
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
  do y=0,2
    read(input_unit,*) oro(:,y)
  enddo
  allocate(uuh(0:2,0:2,nuvzmax), vvh(0:2,0:2,nuvzmax))
  allocate(wwh(0:2,0:2,nwzmax), pvh(0:2,0:2,nuvzmax))
  allocate(rhoh(0:2,0:2,nuvzmax), prsh(0:2,0:2,nuvzmax), pinmconv(0:2,0:2,nzmax))
  allocate(saved_u(0:2,0:2,nz,2,4), saved_v(0:2,0:2,nz,2,4), saved_w(0:2,0:2,nz,2,4))
  open(newunit=output_unit, file=trim(output_path), status='replace', action='write')
  write(output_unit,'(A)') 'FLEXPART_INTERIOR_W_ORACLE_V1'
  write(output_unit,'(A,6(1X,ES24.16E3))') 'GEOMETRY', grid_dx, grid_dy, &
    grid_xlon0, grid_ylat0, physical_dxconst, physical_dyconst
  do memory=1,2
    do y=0,2
    do x=0,2
      read(input_unit,*) surface_p, surface_t, surface_td
      ps(x,y,1,memory)=surface_p; tt2(x,y,1,memory)=surface_t; td2(x,y,1,memory)=surface_td
      do k=1,nz
        read(input_unit,*) tth(x,y,k,memory), qvh(x,y,k,memory), &
          uuh(x,y,k), vvh(x,y,k), wwh(x,y,k)
      enddo
    enddo
    enddo
    pvh=0.
    if (memory == 1) call verttransform_init(memory)
    call verttransform_ecmwf_heights(nxmin1, nymin1, tt2(:,:,1,memory), &
      td2(:,:,1,memory), ps(:,:,1,memory), qvh(:,:,:,memory), tth(:,:,:,memory), &
      prsh, rhoh, pinmconv, etauvheight(:,:,:,memory), etawheight(:,:,:,memory))
    do y=0,2
      do x=0,2
        do k=1,nz
          write(output_unit,'(A,4(1X,I0),8(1X,ES24.16E3))') 'NATIVE', memory, x, y, k, &
            etauvheight(x,y,k,memory), etawheight(x,y,k,memory), prsh(x,y,k), &
            pinmconv(x,y,k), uuh(x,y,k), vvh(x,y,k), wwh(x,y,k), wwh(x,y,k)*pinmconv(x,y,k)
        enddo
      enddo
    enddo
    do variant=1,4
      dxconst=physical_dxconst; dyconst=physical_dyconst
      if (variant==3.or.variant==4) dxconst=0.
      if (variant==2.or.variant==4) dyconst=0.
      write(output_unit,'(A,1X,A,1X,I0,2(1X,ES24.16E3))') 'CONTROL', trim(labels(variant)), memory, dxconst, dyconst
      ! All four controls execute the same unmodified linked upstream routine.
      call verttransform_ecmwf_windfields(memory, nxmin1, nymin1, uuh, vvh, wwh, pvh, rhoh, prsh, pinmconv)
      saved_u(:,:,:,memory,variant)=uu(:,:,:,memory)
      saved_v(:,:,:,memory,variant)=vv(:,:,:,memory)
      saved_w(:,:,:,memory,variant)=ww(:,:,:,memory)
      do y=0,2
        do x=0,2
          do k=1,nz
            write(output_unit,'(A,1X,A,4(1X,I0),4(1X,ES24.16E3))') 'SHARED', trim(labels(variant)), &
              memory, x, y, k, height(k), uu(x,y,k,memory), vv(x,y,k,memory), ww(x,y,k,memory)
          enddo
        enddo
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
    ix=int(query_x); iy=int(query_y)
    terrain_query=(1.-(query_x-ix))*(1.-(query_y-iy))*oro(ix,iy) &
      +(query_x-ix)*(1.-(query_y-iy))*oro(ix+1,iy) &
      +(1.-(query_x-ix))*(query_y-iy)*oro(ix,iy+1)+(query_x-ix)*(query_y-iy)*oro(ix+1,iy+1)
    do variant=1,4
      uu(:,:,:,1:2)=saved_u(:,:,:,:,variant)
      vv(:,:,:,1:2)=saved_v(:,:,:,:,variant)
      ww(:,:,:,1:2)=saved_w(:,:,:,:,variant)
      ! Every sampled source pair belongs to the same controlled variant.
      call interpol_wind(itime, query_x, query_y, z, 0.)
      write(output_unit,'(A,1X,A,2(1X,I0),7(1X,ES24.16E3))') 'QUERY', trim(labels(variant)), query, itime, &
        query_x, query_y, z, z+terrain_query, sampled_u, sampled_v, sampled_w
    enddo
  enddo
  close(input_unit)
  close(output_unit)
end program interior_w_oracle
