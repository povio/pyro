#include <libproc.h>
#include <mach/mach_time.h>
#include <stdio.h>
#include <stdlib.h>
int main(int argc,char **argv) {
    mach_timebase_info_data_t clock;
    if(mach_timebase_info(&clock)!=KERN_SUCCESS) return 2;
    for(int i=1;i<argc;i++) {
        pid_t pid=(pid_t)strtol(argv[i],NULL,10);
        struct rusage_info_v0 data={0};
        if(proc_pid_rusage(pid,RUSAGE_INFO_V0,(rusage_info_t *)&data)) return 3;
        double ns=((double)data.ri_user_time+data.ri_system_time)*clock.numer/clock.denom;
        printf("%d %.0f\n",pid,ns);
    }
    return 0;
}
