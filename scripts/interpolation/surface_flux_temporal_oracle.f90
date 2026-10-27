program surface_flux_temporal_oracle
  ! Direct pinned FLEXPART 11.1 oracle for #120 surface-flux temporal sampling.
  !
  ! readwind_ecmwf stores the flex_extract point values of SSHF and SSR in the
  ! memory slots sshf(:,:,1,n) / ssr(:,:,1,n) (windfields_mod.f90:2351-2368).
  ! EWSS/NSSS exist only as readwind_ecmwf locals before sfcstress is formed
  ! (windfields_mod.f90:2369-2387, 2579), so they use driver arrays with the
  ! identical memory-slot shape. Every value is sampled with the upstream 2-D
  ! surface chain used for ustar/wstar/oli (interpol_mod.f90:697-737):
  ! find_grid_indices, find_grid_distances, find_time_vars, hor_interpol per
  ! memind(m) and temporal_interpolation. No upstream routine is copied.
  use com_mod, only: memtime, memind, numbnests, xglobal, sglobal, nglobal, &
    lcw, lcwsum, ipin, loutrestart
  use par_mod, only: numwfmem
  use windfields_mod, only: nxmax, nymax, nuvzmax, nwzmax, nzmax, nx, ny, &
    nz, nxfield, nxmin1, nymin1, nuvz, nwz, sshf, ssr, alloc_windfields, &
    alloc_fixedfields
  use interpol_mod, only: find_grid_indices, find_grid_distances, find_time_vars, &
    hor_interpol, temporal_interpolation, dt1, dt2, dtt, ngrid
  implicit none
  integer, parameter :: nfield = 4
  integer :: input_unit, output_unit, npair, pair, field, memory, nquery, query, itime
  integer :: grid_nx, grid_ny, valid_time_1, valid_time_2, x, y
  real :: xt, yt
  real :: sampled(2), interpolated(nfield), slots(2, nfield)
  real, allocatable :: memory_values(:), stress_east(:,:,:,:), stress_north(:,:,:,:)
  character(len=1024) :: input_path, output_path

  call get_command_argument(1, input_path)
  call get_command_argument(2, output_path)
  open(newunit=input_unit, file=trim(input_path), status='old', action='read')
  read(input_unit,*) grid_nx, grid_ny
  if (grid_nx < 2 .or. grid_ny < 2) error stop 'at least a 2x2 grid is required'
  nxmax=grid_nx; nymax=grid_ny; nx=grid_nx; ny=grid_ny; nxfield=grid_nx
  nxmin1=grid_nx-1; nymin1=grid_ny-1
  nuvzmax=2; nwzmax=2; nzmax=2; nuvz=2; nwz=2; nz=2
  numbnests=0; ngrid=0; xglobal=.false.; sglobal=.false.; nglobal=.false.
  lcw=.false.; lcwsum=.false.; ipin=0; loutrestart=-1
  memind(1)=1; memind(2)=2
  call alloc_fixedfields
  call alloc_windfields
  allocate(memory_values(grid_nx*grid_ny))
  allocate(stress_east(0:nxmax-1,0:nymax-1,1,numwfmem))
  allocate(stress_north(0:nxmax-1,0:nymax-1,1,numwfmem))

  open(newunit=output_unit, file=trim(output_path), status='replace', action='write')
  write(output_unit,'(A)') 'FLEXPART_SURFACE_FLUX_TEMPORAL_ORACLE_V1'
  write(output_unit,'(A,2(1X,I0))') 'GRID', grid_nx, grid_ny
  read(input_unit,*) npair
  do pair=1,npair
    read(input_unit,*) valid_time_1, valid_time_2
    if (valid_time_2 <= valid_time_1) error stop 'memtime(2) must follow memtime(1)'
    memtime(1)=valid_time_1; memtime(2)=valid_time_2
    write(output_unit,'(A,3(1X,I0))') 'PAIR', pair, memtime(1), memtime(2)
    ! Canonical x-fastest values, one line per field and memory slot.
    do field=1,nfield
      do memory=1,2
        read(input_unit,*) memory_values
        do y=0,grid_ny-1
          do x=0,grid_nx-1
            select case (field)
            case (1)
              sshf(x,y,1,memind(memory))=memory_values(1+x+y*grid_nx)
            case (2)
              ssr(x,y,1,memind(memory))=memory_values(1+x+y*grid_nx)
            case (3)
              stress_east(x,y,1,memind(memory))=memory_values(1+x+y*grid_nx)
            case (4)
              stress_north(x,y,1,memind(memory))=memory_values(1+x+y*grid_nx)
            end select
          enddo
        enddo
      enddo
    enddo
    read(input_unit,*) nquery
    do query=1,nquery
      read(input_unit,*) itime, xt, yt
      call find_grid_indices(xt,yt)
      call find_grid_distances(xt,yt)
      call find_time_vars(itime)
      do field=1,nfield
        do memory=1,2
          select case (field)
          case (1)
            call hor_interpol(sshf,sampled(memory),1,memind(memory),1)
          case (2)
            call hor_interpol(ssr,sampled(memory),1,memind(memory),1)
          case (3)
            call hor_interpol(stress_east,sampled(memory),1,memind(memory),1)
          case (4)
            call hor_interpol(stress_north,sampled(memory),1,memind(memory),1)
          end select
          slots(memory,field)=sampled(memory)
        enddo
        call temporal_interpolation(sampled(1),sampled(2),interpolated(field))
      enddo
      write(output_unit,'(A,3(1X,I0),2(1X,ES24.16E3))') 'QUERY', pair, query, itime, xt, yt
      write(output_unit,'(A,3(1X,ES24.16E3))') 'DTS', dt1, dt2, dtt
      do field=1,nfield
        write(output_unit,'(A,1X,I0,3(1X,ES24.16E3))') 'FIELD', field, &
          slots(1,field), slots(2,field), interpolated(field)
      enddo
    enddo
  enddo
  close(input_unit)
  close(output_unit)
end program surface_flux_temporal_oracle
