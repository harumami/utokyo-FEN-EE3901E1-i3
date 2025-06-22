use {
    hypha_py as _,
    pyo3_stub_gen::generate::StubInfo,
    std::env::current_dir,
};

fn main() {
    StubInfo::from_project_root("hypha_py".to_owned(), current_dir().unwrap())
        .unwrap()
        .generate()
        .unwrap();
}
