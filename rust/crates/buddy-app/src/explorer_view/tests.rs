use super::*;
use std::fs;

#[test]
fn listings_filter_hidden_and_generated_directories_and_sort_folders_first() {
    let root = tempfile::tempdir().unwrap();
    for name in ["zeta", "Alpha", "node_modules", "__pycache__", ".git"] {
        fs::create_dir(root.path().join(name)).unwrap();
    }
    for name in ["b.txt", "A.txt", ".secret", "node_modules.txt"] {
        fs::write(root.path().join(name), "").unwrap();
    }
    let entries = files::directory(root.path()).unwrap();
    assert_eq!(
        entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["Alpha", "zeta", "A.txt", "b.txt", "node_modules.txt"]
    );
    assert!(files::directory(&root.path().join("missing")).is_err());
}

#[test]
fn preview_handles_empty_unicode_invalid_utf8_binary_large_and_missing_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("sample.TS");
    fs::write(&path, "const 界 = 1;\n").unwrap();
    let data = files::preview(&path).unwrap();
    assert_eq!(data.language, "typescript");
    assert_eq!(data.size, "const 界 = 1;\n".len());
    fs::write(&path, "").unwrap();
    assert!(files::preview(&path).unwrap().content.is_empty());
    fs::write(&path, [0xff, b'a']).unwrap();
    assert_eq!(files::preview(&path).unwrap().content, "�a");
    fs::write(&path, vec![b'a'; files::MAX_SIZE as usize]).unwrap();
    assert_eq!(
        files::preview(&path).unwrap().size,
        files::MAX_SIZE as usize
    );
    fs::write(&path, vec![b'a'; files::MAX_SIZE as usize + 1]).unwrap();
    assert!(
        files::preview(&path)
            .err()
            .unwrap()
            .contains("File too large")
    );
    fs::write(root.path().join("image.PNG"), "fake").unwrap();
    assert!(
        files::preview(&root.path().join("image.PNG"))
            .err()
            .unwrap()
            .contains("Binary")
    );
    assert!(files::preview(&root.path().join("missing")).is_err());
    assert!(files::preview(root.path()).is_err());
}

#[test]
fn language_and_size_match_electron() {
    for (name, expected) in [
        ("Dockerfile", "dockerfile"),
        ("Makefile", "makefile"),
        (".env", "shell"),
        (".gitignore", "gitignore"),
        ("code.PSM1", "powershell"),
        ("a.gql", "graphql"),
        ("no-extension", "plaintext"),
    ] {
        assert_eq!(files::language(std::path::Path::new(name)), expected);
    }
    assert_eq!(files::size(1), "1 B");
    assert_eq!(files::size(1536), "1.5 KB");
    assert_eq!(files::size(1048576), "1.0 MB");
}

#[gpui_kit::test]
fn lazy_expansion_caches_children_and_new_roots_reset_selection(cx: &mut gpui_kit::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("nested")).unwrap();
    fs::write(root.path().join("nested/code.rs"), "fn main() {}\n").unwrap();
    let view = cx.new(|_| ExplorerView::new());
    view.update(cx, |view, cx| view.open(root.path().into(), cx));
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        assert_eq!(view.children.len(), 1);
        assert!(!view.children.contains_key(&root.path().join("nested")));
        view.toggle(root.path().join("nested"), cx);
        view.toggle(root.path().join("nested"), cx);
        assert_eq!(view.pending.len(), 1);
    });
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        assert!(!view.expanded.contains(&root.path().join("nested")));
        assert!(view.children.contains_key(&root.path().join("nested")));
        view.toggle(root.path().join("nested"), cx);
        assert!(view.pending.is_empty());
        view.selected = Some(root.path().join("nested/code.rs"));
        view.open(root.path().join("nested"), cx);
        assert!(view.selected.is_none());
        assert!(view.children.is_empty());
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert_eq!(
            view.children[&root.path().join("nested")][0].name,
            "code.rs"
        )
    });
}

#[gpui_kit::test]
fn a_superseded_root_read_cannot_populate_the_new_tree(cx: &mut gpui_kit::TestAppContext) {
    let old = tempfile::tempdir().unwrap();
    let new = tempfile::tempdir().unwrap();
    fs::write(old.path().join("old.txt"), "old").unwrap();
    fs::write(new.path().join("new.txt"), "new").unwrap();
    let view = cx.new(|_| ExplorerView::new());
    view.update(cx, |view, cx| {
        view.open(old.path().into(), cx);
        view.open(new.path().into(), cx);
    });
    cx.run_until_parked();
    view.update(cx, |view, _| {
        assert_eq!(view.children.len(), 1);
        assert_eq!(view.children[new.path()][0].name, "new.txt");
    });
}

#[gpui_kit::test]
fn rapid_selection_keeps_the_latest_file_and_failure_clears_old_content(
    cx: &mut gpui_kit::TestAppContext,
) {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.rs");
    let last = root.path().join("last.rs");
    fs::write(&first, "old").unwrap();
    fs::write(&last, "fn latest() {}\n").unwrap();
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        let mut view = None;
        let (window, _) = gpui_kit::open_window(Default::default(), cx, |window, cx| {
            let entity = cx.new(|_| ExplorerView::new());
            entity.update(cx, |view, cx| {
                view.select(first, window, cx);
                view.select(last.clone(), window, cx);
            });
            view = Some(entity.clone());
            entity
        })
        .unwrap();
        (window, view.unwrap())
    });
    let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    view.update(&mut visual, |view, cx| {
        assert_eq!(view.selected, Some(last));
        assert_eq!(
            view.preview.as_ref().unwrap().as_ref().unwrap().content,
            "fn latest() {}\n"
        );
        assert_eq!(
            view.editor.as_ref().unwrap().read(cx).value(),
            "fn latest() {}\n"
        );
    });
    let editor = view.read_with(&visual, |view, _| view.editor.clone().unwrap());
    visual.update(|window, cx| editor.update(cx, |input, cx| input.focus(window, cx)));
    visual.simulate_keystrokes("x");
    assert_eq!(
        editor.read_with(&visual, |input, _| input.value()),
        "fn latest() {}\n"
    );
    visual.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.select(root.path().join("missing.txt"), window, cx);
            assert!(view.editor.is_none());
        })
    });
    visual.run_until_parked();
    view.update(&mut visual, |view, _| {
        assert!(view.preview.as_ref().unwrap().is_err());
        assert!(view.editor.is_none());
    });
}

#[gpui_kit::test]
fn a_folder_can_be_opened_by_path_and_enter_without_an_os_dialog(
    cx: &mut gpui_kit::TestAppContext,
) {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("visible.txt"), "fixture").unwrap();
    let (window, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::install(&buddy_core::config::AppConfig::default(), cx);
        gpui_kit::open_window(Default::default(), cx, |window, cx| {
            cx.new(|cx| {
                let mut view = ExplorerView::new();
                view.choose(window, cx);
                view
            })
        })
        .unwrap()
    });
    let mut visual = gpui_kit::VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");
    view.read_with(&visual, |view, _| assert!(view.chooser_error.is_some()));
    let input = view.read_with(&visual, |view, _| view.chooser.clone().unwrap());
    visual.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_value(root.path().display().to_string(), window, cx)
        })
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    view.read_with(&visual, |view, _| {
        assert!(view.chooser.is_none());
        assert_eq!(view.root, root.path());
        assert_eq!(view.children[root.path()][0].name, "visible.txt");
    });
}
