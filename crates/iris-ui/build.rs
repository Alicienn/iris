fn main() {
    // Le style « fluent » sert de base aux widgets standard ; tout le reste est
    // dessiné par nos propres composants, à partir des tokens.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compilation de l'interface");
}
