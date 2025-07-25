use ::hypha_lpc_sys::{
    N,
    P,
    decode as raw_decode,
    encode as raw_encode,
};

pub fn encode(values: [f32; N as _]) -> [f32; (P + N) as _] {
    let mut coded = [0.; (P + N) as _];
    let (buffer0, buffer1) = coded.split_at_mut(5);
    buffer1.copy_from_slice(&values);
    unsafe { raw_encode(buffer0.as_mut_ptr(), buffer1.as_mut_ptr()) };
    coded
}

pub fn decode(coded: [f32; (P + N) as _]) -> [f32; N as _] {
    let mut values = [0.; N as _];
    let (buffer0, buffer1) = coded.split_at(5);
    values.copy_from_slice(buffer1);
    unsafe { raw_decode(buffer0.as_ptr().cast_mut(), values.as_mut_ptr()) };
    values
}
