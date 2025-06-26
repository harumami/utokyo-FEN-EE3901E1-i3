#ifndef HYPHA_LPC_LPC_H
#define HYPHA_LPC_LPC_H

#define P 5
#define N 20

#ifdef __cplusplus
extern "C" {
#endif

void encode(float p_as[P], float p_ss[N]);
void decode(const float p_as[P], float p_es[N]);

#ifdef __cplusplus
}
#endif

#endif
