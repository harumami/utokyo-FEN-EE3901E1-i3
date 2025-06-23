#include <stdint.h>
#include "LP.h"


//配列長20の16bit符号付き整数配列のポインタをraw_dataにいれ、encode(data)でdataがLPC符号化される
//encoded_dataには配列長25の不動点小数配列のポインタをいれる。
//実行後のencoded_dataを送る。
void encode(int16_t *raw_data,float *encoded_data){
    float a[5];
    float data[20];
    for(int i = 0; i < 20; i++){
        data[i] = (float)raw_data[i];
    }
    toLPC(a,data);
    memcpy(encoded_data, a, sizeof(float) * 5);
    memcpy(encoded_data + 5, data, sizeof(float) * 20);
}

//送られてきたデータをencoded_dataに入れる。
//予め作成した配列長20の不動点小数配列を用意しておき、decoded_dataに入れる。
//実行後のdecoded_dataを受け取る。
void decode(float *encoded_data,int16_t *decoded_data){
    float a[5];
    float data[20];
    memcpy(a, encoded_data, sizeof(float) * 5);
    memcpy(data, encoded_data + 5, sizeof(float) * 20);
    fromLPC(a, data);
    for(int i = 0; i < 20; i++){
        decoded_data[i] = (int16_t)data[i];
    }
}

