#include <Eigen/Dense>
#include <cmath>

constexpr const int p = 5;
constexpr const int N = 20;

// 配列長20の16bit符号付き整数配列のポインタをraw_dataにいれ、encode(data)でdataがLPC符号化される
// encoded_dataには配列長25の不動点小数配列のポインタをいれる。
// 実行後のencoded_dataを送る。
extern "C" void encode(float (&p_as)[p], float (&p_ss)[N]) {
    Eigen::Vector<float, p + 1> r;

    for (int i = 0; i < p + 1; i++) {
        float sum = 0;

        for (int n = i; n < N; n++) {
            if (n >= i) {
                sum += p_ss[n] * p_ss[n - i];
            }
        }

        r(i) = sum;
    }

    Eigen::Matrix<float, p, p> R;

    for (int i = 0; i < p; i++) {
        for (int j = 0; j < p; j++) {
            R(i, j) = r(abs(i - j));
        }
    }

    Eigen::Vector<float, p> minus_a = R.inverse() * r.tail(p);
    float e[N];

    for (int n = 0; n < N; n++) {
        int as = 0;

        for (int i = 0; i < p; i++) {
            if (n > i) {
                as += static_cast<int>(minus_a(i) * p_ss[n - i - 1]);
            }
        }

        e[n] = p_ss[n] - static_cast<float>(as);
    }

    std::copy_n(minus_a.data(), p, &p_as[0]);
    std::copy_n(&e[0], N, &p_ss[0]);
}

// 送られてきたデータをencoded_dataに入れる。
// 予め作成した配列長20の不動点小数配列を用意しておき、decoded_dataに入れる。
// 実行後のdecoded_dataを受け取る。
extern "C" void decode(float (&p_as)[p], float (&p_es)[N]) {
    float s[N];

    for (int n = 0; n < N; n++) {
        int as = 0;

        for (int i = 0; i < p; i++) {
            if (n > i) {
                as += static_cast<int>(p_as[i] * s[n - i - 1]);
            }
        }

        s[n] = p_es[n] + static_cast<float>(as);
    }

    std::copy_n(&s[0], N, &p_es[0]);
}
