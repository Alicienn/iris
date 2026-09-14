fn main() {
    // Le style « fluent » sert de base aux widgets standard ; tout le reste est
    // dessiné par nos propres composants, à partir des tokens.
    let mut config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());

    // Les informations de débogage sont ce qui permet aux tests de retrouver un
    // élément par son libellé d'accessibilité. Elles ne sont émises que dans les
    // profils de développement : en production elles n'ont pas d'usage, et elles
    // gonflent le binaire de la structure complète de l'interface.
    if std::env::var("DEBUG").as_deref() == Ok("true") {
        config = config.with_debug_info(true);
    }

    slint_build::compile_with_config("ui/app.slint", config).expect("compilation de l'interface");
    println!("cargo:rerun-if-env-changed=DEBUG");
}
