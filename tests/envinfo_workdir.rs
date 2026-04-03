use std::path::Path;

use lumos::envinfo::shorten_home_prefix;

#[test]
fn shorten_home_prefix_only_replaces_a_real_home_prefix() {
    let cases = [
        ("/home/archie", "/home/archie", "~"),
        ("/home/archie/project", "/home/archie", "~/project"),
        (
            "/tmp/home/archie/project",
            "/home/archie",
            "/tmp/home/archie/project",
        ),
        (
            "/home/archie-dev/project",
            "/home/archie",
            "/home/archie-dev/project",
        ),
    ];

    for (working_dir, home_dir, expected) in cases {
        let shortened = shorten_home_prefix(Path::new(working_dir), Path::new(home_dir));
        assert_eq!(shortened, expected);
    }
}
