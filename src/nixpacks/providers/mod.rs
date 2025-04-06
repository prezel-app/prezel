use astro::AstroProvider;
use nixpacks::providers::{
    clojure::ClojureProvider, cobol::CobolProvider, crystal::CrystalProvider,
    csharp::CSharpProvider, dart::DartProvider, deno::DenoProvider, elixir::ElixirProvider,
    fsharp::FSharpProvider, gleam::GleamProvider, go::GolangProvider,
    haskell::HaskellStackProvider, java::JavaProvider, lunatic::LunaticProvider,
    node::NodeProvider, php::PhpProvider, python::PythonProvider, ruby::RubyProvider,
    rust::RustProvider, scala::ScalaProvider, staticfile::StaticfileProvider, swift::SwiftProvider,
    zig::ZigProvider, Provider,
};

mod astro;

pub fn get_providers() -> &'static [&'static (dyn Provider)] {
    &[
        &AstroProvider,
        &CrystalProvider {},
        &CSharpProvider {},
        &DartProvider {},
        &ElixirProvider {},
        &DenoProvider {},
        &FSharpProvider {},
        &ClojureProvider {},
        &GleamProvider {},
        &GolangProvider {},
        &HaskellStackProvider {},
        &JavaProvider {},
        &LunaticProvider {},
        &ScalaProvider {},
        &PhpProvider {},
        &RubyProvider {},
        &NodeProvider {},
        &PythonProvider {},
        &RustProvider {},
        &SwiftProvider {},
        &StaticfileProvider {},
        &ZigProvider {},
        &CobolProvider {},
    ]
}
