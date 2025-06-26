#include <Eigen/Dense>
#include <cmath>
#include <hypha-lpc/LPC.h>
#include <span>

// 配列長20の16bit符号付き整数配列のポインタをraw_dataにいれ、encode(data)でdataがLPC符号化される
// encoded_dataには配列長25の不動点小数配列のポインタをいれる。
// 実行後のencoded_dataを送る。
extern "C" void encode(float p_as[P], float p_ss[N]) {
    std::span<float, P> as_span(p_as, P);
    std::span<float, N> ss_span(p_ss, N);
    Eigen::Vector<float, P + 1> r;

    for (size_t i = 0; i < P + 1; i++) {
        float sum = 0;

        for (size_t n = i; n < N; n++) {
            if (n >= i) {
                sum += ss_span[n] * ss_span[n - i];
            }
        }

        r(static_cast<int>(i)) = sum;
    }

    Eigen::Matrix<float, P, P> R;

    for (int i = 0; i < P; i++) {
        for (int j = 0; j < P; j++) {
            R(i, j) = r(abs(i - j));
        }
    }

    Eigen::Vector<float, P> minus_a = R.inverse() * r.tail(P);
    std::array<float, N> e{};

    for (size_t n = 0; n < N; n++) {
        int as = 0;

        for (size_t i = 0; i < P; i++) {
            if (n > i) {
                as += static_cast<int>(
                    minus_a(static_cast<int>(i)) * ss_span[n - i - 1]
                );
            }
        }

        e[n] = ss_span[n] - static_cast<float>(as);
    }

    std::copy_n(minus_a.data(), P, as_span.data());
    std::copy_n(e.data(), N, ss_span.data());
}

// 送られてきたデータをencoded_dataに入れる。
// 予め作成した配列長20の不動点小数配列を用意しておき、decoded_dataに入れる。
// 実行後のdecoded_dataを受け取る。
extern "C" void decode(const float p_as[P], float p_es[N]) {
    std::span<const float, P> as_span(p_as, P);
    std::span<float, N> es_span(p_es, N);
    std::array<float, N> s{};

    for (size_t n = 0; n < N; n++) {
        int as = 0;

        for (size_t i = 0; i < P; i++) {
            if (n > i) {
                as += static_cast<int>(as_span[i] * s[n - i - 1]);
            }
        }

        s[n] = es_span[n] + static_cast<float>(as);
    }

    std::copy_n(s.data(), N, es_span.data());
}
