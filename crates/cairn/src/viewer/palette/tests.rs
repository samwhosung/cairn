use super::*;

#[test]
fn the_palette_is_asked_in_words() {
    assert_eq!(parse(&[]), Ok(Ask::Say));
    assert_eq!(parse(&["open"]), Ok(Ask::Open(true)));
    assert_eq!(parse(&["close"]), Ok(Ask::Open(false)));
    assert_eq!(parse(&["tab", "tree"]), Ok(Ask::Tab("tree".into())));
    assert_eq!(
        parse(&["search", "elwynn", "tree"]),
        Ok(Ask::Search("elwynn tree".into()))
    );
    assert_eq!(parse(&["search"]), Ok(Ask::Search(String::new())));
    assert_eq!(parse(&["order", "plain"]), Ok(Ask::Order(Order::Plain)));
    assert_eq!(parse(&["size", "64"]), Ok(Ask::Size(64.0)));
    assert_eq!(parse(&["follow", "off"]), Ok(Ask::Follow(false)));
    assert_eq!(parse(&["pick", "3"]), Ok(Ask::Pick(3)));
    assert_eq!(
        parse(&["list", "a.txt"]),
        Ok(Ask::List {
            out: "a.txt".into(),
            top: FIRST_PAGE
        })
    );
    assert_eq!(
        parse(&["list", "a.txt", "--top", "5"]),
        Ok(Ask::List {
            out: "a.txt".into(),
            top: 5
        })
    );
    assert_eq!(parse(&["frames", "30"]), Ok(Ask::Frames(30)));
    for wrong in [
        &["pick", "0"][..],
        &["size", "-1"],
        &["order", "best"],
        &["list", "a.txt", "--top", "0"],
        &["frames", "x"],
        &["tab"],
        &["shut"],
    ] {
        assert!(parse(wrong).is_err(), "{wrong:?}");
    }
}
