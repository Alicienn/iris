fn main() {
    // Compilé sur un fil à grande pile, et non sur celui que Windows nous donne.
    //
    // Le compilateur Slint descend l'arbre des composants par récursion, et l'interface
    // d'Iris est profonde : la fenêtre contient des panneaux qui contiennent des listes
    // qui contiennent des lignes qui contiennent des boutons. Sur le mégaoctet que
    // Windows accorde par défaut au fil principal d'un exécutable, la descente déborde
    // — sans message, sans ligne de journal, juste `STATUS_STACK_OVERFLOW` et un code de
    // sortie hexadécimal que rien ne relie à l'interface.
    //
    // `RUST_MIN_STACK` ne s'applique pas ici : elle ne dimensionne que les fils créés
    // ensuite, jamais le principal, dont la taille est fixée à l'édition de liens. D'où
    // ce fil-ci, qui est le seul endroit où l'on puisse la choisir.
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(compiler)
        .expect("fil de compilation")
        .join()
        .expect("la compilation de l'interface a débordé sa pile");
}

fn compiler() {
    // Le style « fluent » sert de base aux widgets standard ; tout le reste est
    // dessiné par nos propres composants, à partir des tokens.
    let mut config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());

    // Every import names its file from the root of the interface, `@iris/…`, and not
    // from where the importing file happens to sit: the files live in layer folders
    // (theme, base, controls, lists, layout, screens, shell), and a path relative to
    // each of them would change every time one moves.
    let racine = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    config = config.with_library_paths(std::collections::HashMap::from([(
        "iris".to_string(),
        racine,
    )]));

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
