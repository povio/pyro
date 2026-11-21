#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
int pyro_mlkem_generate(int,unsigned char*,size_t,unsigned char*,size_t);
int pyro_mlkem_convert(int,int,int,const unsigned char*,size_t,unsigned char*,size_t,size_t*);
int pyro_mlkem_encapsulate(int,const unsigned char*,size_t,unsigned char*,size_t,unsigned char*,size_t);
int pyro_mlkem_decapsulate(int,const unsigned char*,size_t,const unsigned char*,size_t,unsigned char*,size_t);
void pyro_mlkem_cleanse(unsigned char*,size_t);
static void test(int lvl,size_t pl,size_t cl){
 unsigned char seed[64],pub[1568],ct[1568],shared[32],recovered[32],der[2048],rt[1568];
 size_t sp=0,pk=0,n=0;
 if(!pyro_mlkem_generate(lvl,seed,64,pub,pl)||
 !pyro_mlkem_encapsulate(lvl,pub,pl,ct,cl,shared,32)||
 !pyro_mlkem_decapsulate(lvl,seed,64,ct,cl,recovered,32)||memcmp(shared,recovered,32)||
 !pyro_mlkem_convert(lvl,1,3,pub,pl,der,sizeof der,&sp)||
 !pyro_mlkem_convert(lvl,3,1,der,sp,rt,sizeof rt,&n)||n!=pl||memcmp(pub,rt,n)||
 !pyro_mlkem_convert(lvl,2,4,seed,64,der,sizeof der,&pk)||
 !pyro_mlkem_convert(lvl,4,2,der,pk,rt,sizeof rt,&n)||n!=64||memcmp(seed,rt,64)){
 fprintf(stderr,"roundtrip failed at ML-KEM-%d\n",lvl);exit(1);}
 printf("{\"level\":%d,\"spkiBytes\":%zu,\"pkcs8Bytes\":%zu,\"seedRoundTrip\":true,\"publicRoundTrip\":true,\"agreement\":true}\n",lvl,sp,pk);
 pyro_mlkem_cleanse(seed,sizeof seed);pyro_mlkem_cleanse(der,sizeof der);pyro_mlkem_cleanse(rt,sizeof rt);pyro_mlkem_cleanse(shared,32);pyro_mlkem_cleanse(recovered,32);
}
int main(){test(512,800,768);test(768,1184,1088);test(1024,1568,1568);}
