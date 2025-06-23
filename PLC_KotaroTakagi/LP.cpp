#include <Eigen/Dense>
#include <iostream>
#include <stdio.h>
#include <cstdlib>

using namespace Eigen;
# define p 5
# define N 20

extern "C" {
    void toLP(float *p_as, float *p_ss){
        
        Vector<float,p+1> r;
        for (int i = 0; i < p+1; i++){
            float sum = 0;
            for (int n = i; n < N; n++) {
                if ( n >= i){
                    sum += p_ss[n] * p_ss[n-i];
                }
            }
            r(i) = sum;
        }
        Matrix<float, p,p> R;
        for (int i = 0; i < p; i++) {
            for (int j = 0; j < p; j++) {
                R(i,j) = r(abs(i - j));
            }
        }
        Vector<float, p> minus_a = R.inverse() * r.tail(p);
        float e[N];
        for(int n = 0; n < N; n++ ){
            int as = 0;
            for (int i = 0; i < p; i++){
                if ( n >i){
                    as += minus_a(i)*p_ss[n -i -1];
                }
            }
            e[n] = p_ss[n] - as;
        }

        memcpy(p_as,minus_a.data(),sizeof(float)*p);
        memcpy(p_ss,e,sizeof(float)*N);
        

    }

    void fromLP(float *p_as, float *p_es){
        float s[N];
        for(int n =0; n < N; n++){
            int as = 0;
            for (int i = 0; i < p; i++){
                if ( n > i){
                    as += p_as[i]*s[n -i -1];
                }
            }
            s[n] = p_es[n] + as;
        }
        memcpy(p_es,s,sizeof(float)*N);
    }

    void print_p(float *p_ps,char *str, int len){
        std::cout << str;
        for(int i = 0; i < len; i++){
            std::cout << " " << p_ps[i];
        }
        std::cout << std::endl;
    }
}
