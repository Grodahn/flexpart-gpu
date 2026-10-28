program top_boundary_oracle
  ! #213 additive gas-carrier version of the frozen #180 scenarios 1-8.
  ! All transport and boundary arithmetic is unchanged upstream code.
  use iso_c_binding, only: c_int
  use flagged_top_observer, only: observation_unit, observation_id, stage_clock, emit_observation
  use par_mod, only: dp
  use com_mod
  use point_mod, only: grid_dx => dx, grid_dy => dy, grid_xlon0 => xlon0, grid_ylat0 => ylat0
  use windfields_mod
  use verttransform_mod, only: verttransform_init, verttransform_ecmwf_heights
  use interpol_mod, only: alloc_interpol
  use random_mod, only: alloc_random
  use particle_mod, only: part, particle
  use advance_mod, only: advance
  implicit none
  ! GNU objcopy exports this private symbol without changing its machine code.
  ! The normal public advance entry uses the same original object instructions.
  interface
    subroutine direct_push(ipart) bind(C, name='__advance_mod_MOD_pushpartdown')
      import c_int
      integer(c_int), intent(in) :: ipart
    end subroutine
  end interface
  integer :: inp, out, layers, k, x, m, geometry, direction, settling, side, scenario, epoch
  integer :: itime, row_id
  real :: a(4), b(4), surface_p, surface_t, surface_td, ignored(3)
  real :: prsh(0:1,0:1,4), rhoh(0:1,0:1,4), conversion(0:1,0:1,4)
  real :: start_z, old_w, end_w, slope, fraction
  real :: base_ps(2,2), base_tt2(2,2), base_td2(2,2), base_t(4,2,2), base_q(4,2,2)
  character(len=1024) :: input_path, output_path

  call get_command_argument(1,input_path)
  call get_command_argument(2,output_path)
  open(newunit=inp,file=trim(input_path),status='old')
  read(inp,*) layers
  if (layers /= 3) error stop 'requires existing #118 three-layer input'
  nxmax=2; nymax=2; nx=2; ny=2; nxfield=2; nxmin1=1; nymin1=1
  nuvzmax=4; nwzmax=4; nzmax=4; nuvz=4; nwz=4; nz=4
  numbnests=0; xglobal=.false.; sglobal=.false.; nglobal=.false.
  lcw=.false.; lcwsum=.false.; ipin=0; loutrestart=-1
  grid_dx=1.; grid_dy=1.; grid_xlon0=0.; grid_ylat0=0.
  dxconst=1./111200.; dyconst=1./111200.
  memind(1)=1; memind(2)=2; memind(3)=1
  DRYDEP=.false.; lturbulence=0; lmesoscale_turb=.false.; mdomainfill=0
  numthreads=1; nspec=1; maxspec=1
  call alloc_fixedfields
  call alloc_windfields
  call alloc_com
  call alloc_interpol
  call alloc_random(1)
  allocate(part(1),xmass(1,1))
  xmass=1.; density=-1.; dquer=10.; cunningham=1.; vsetaver=-0.01; ishape=0
  do k=1,4
    read(inp,*) a(k),b(k)
  enddo
  akm=a; bkm=b; akz=0.; bkz=0.; bkz(1)=1.
  do k=2,4
    akz(k)=0.5*(a(k-1)+a(k)); bkz(k)=0.5*(b(k-1)+b(k))
  enddo
  aknew=akz; bknew=bkz
  read(inp,*) oro(0,0),oro(1,0)
  oro(:,1)=oro(:,0)
  do m=1,2
    do x=1,2
      read(inp,*) base_ps(x,m),base_tt2(x,m),base_td2(x,m)
      do k=1,4
        read(inp,*) base_t(k,x,m),base_q(k,x,m),ignored
      enddo
    enddo
  enddo
  close(inp)
  open(newunit=out,file=trim(output_path),status='replace')
  write(out,'(A)') 'FLEXPART_TOP_BOUNDARY_ORACLE_V2'
  observation_unit=out
  row_id=0
  do geometry=0,1
    ! Two independent synthetic runs: equal profiles, then unchanged #118 profiles.
    do m=1,2
      do x=0,1
        if (geometry==0) then
          surface_p=base_ps(1,1); surface_t=base_tt2(1,1); surface_td=base_td2(1,1)
          tth(x,:,:,m)=spread(base_t(:,1,1),1,2)
          qvh(x,:,:,m)=spread(base_q(:,1,1),1,2)
        else
          surface_p=base_ps(x+1,m); surface_t=base_tt2(x+1,m); surface_td=base_td2(x+1,m)
          tth(x,:,:,m)=spread(base_t(:,x+1,m),1,2)
          qvh(x,:,:,m)=spread(base_q(:,x+1,m),1,2)
        endif
        ps(x,:,1,m)=surface_p; tt2(x,:,1,m)=surface_t; td2(x,:,1,m)=surface_td
      enddo
      if (m==1) call verttransform_init(m)
      call verttransform_ecmwf_heights(1,1,tt2(:,:,1,m),td2(:,:,1,m),ps(:,:,1,m), &
        qvh(:,:,:,m),tth(:,:,:,m),prsh,rhoh,conversion,etauvheight(:,:,:,m),etawheight(:,:,:,m))
      do x=0,1
        write(out,'(A,3(1X,I0),4(1X,ES25.17E3))') 'GRID',geometry,m,x,height(nz), &
          etauvheight(x,0,nz,m),ps(x,0,1,m),height(2)
      enddo
    enddo
    do direction=-1,1,2
      ldirect=direction
      do settling=0,1
        lsettling=settling==1
        do side=-1,1
          call reset_particle()
          part(1)%z=real(height(nz),dp)+real(side,dp)*0.0001_dp
          call direct_push(1)
          write(out,'(A,4(1X,I0),1X,ES25.17E3,1X,L1)') &
            'PUSH',geometry,direction,settling,side,part(1)%z,part(1)%nstop
        enddo
      enddo
      do epoch=0,1
        memtime(1)=0; memtime(2)=3600*direction
        itime=1200*epoch*direction
        lsynctime=600*direction
        do scenario=1,8
          lsettling=.true.
          start_z=height(nz)-1.
          old_w=0.; end_w=0.; slope=0.
          select case(scenario)
          case(1)
            start_z=height(nz)-1000.
          case(2)
            start_z=height(nz)
          case(3)
            start_z=height(nz)+1.
          case(4)
            old_w=0.01; end_w=-0.01
          case(5)
            end_w=0.02
          case(6)
            old_w=0.01; end_w=0.01
          case(7)
            old_w=0.01; end_w=0.03
          case(8)
            start_z=height(nz)-1000.; old_w=0.01; end_w=0.02; slope=0.004
          end select
          ! Explicit synthetic shared-grid forcing, not another remapping proof.
          ! Values at current/predicted times and positions differ independently.
          uu=real(direction)*0.5/(600.*dxconst); vv=0.
          do m=1,2
            fraction=real(memtime(m)-itime)/real(lsynctime)
            do x=0,1
              ww(x,:,:,m)=real(direction)*(old_w+(end_w-old_w)*fraction+slope*real(x))
            enddo
          enddo
          tt=280.; rho=1.; hmix=100.; tropopause=12000.
          call reset_particle(); part(1)%xlon=0.25_dp; part(1)%ylat=0._dp
          part(1)%z=real(start_z,dp); part(1)%npoint=1; part(1)%idt=600
          row_id=row_id+1
          observation_id=row_id; stage_clock=itime
          write(out,'(A,5(1X,I0),5(1X,ES25.17E3),2(1X,I0),1X,L1)') &
            'CARRIER',row_id,part(1)%npoint,nspec,1,ishape(1),density(1),dquer(1), &
            cunningham(1),vsetaver(1),xmass(1,1),mdomainfill,ipin,lsettling
          call emit_observation('INPUT')
          call advance(itime,1,0)
          call emit_observation('FINAL')
          observation_id=0
          write(out,'(A,5(1X,I0),8(1X,ES25.17E3),1X,L1)') &
            'ADVANCE',row_id,geometry,direction,epoch,scenario,real(start_z,dp), &
            real(old_w,dp),real(end_w,dp),real(slope,dp),part(1)%xlon,part(1)%ylat, &
            part(1)%z,real(part(1)%settling,dp),part(1)%nstop
        enddo
      enddo
    enddo
  enddo
  close(out)
contains
  subroutine reset_particle()
    ! Initialize only state consumed by the disabled-turbulence production route.
    part(1)%xlon=0.25_dp; part(1)%ylat=0._dp; part(1)%z=0._dp
    part(1)%nstop=.false.; part(1)%settling=0.; part(1)%npoint=1; part(1)%idt=600
  end subroutine
end program top_boundary_oracle
