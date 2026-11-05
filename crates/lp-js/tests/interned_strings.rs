//! `Scope::interned_string`: one weakly held JavaScript string per interned
//! host string, per realm.

use lp_js::*;

fn realm() -> (Engine, Realm) {
    let e = Engine::new().expect("engine");
    let r = e.create_realm(RealmOptions::default()).unwrap();
    (e, r)
}

/// A host string with a reusable identity, as a dynamic atom has (its
/// entry's address): here an index the test hands out.
fn read<'s>(s: &Scope<'s>, id: u64, text: &str) -> Value<'s> {
    s.interned_string((id, 7), text, true)
}

#[test]
fn reads_return_one_string_while_it_is_referenced() {
    let (e, r) = realm();
    {
        let s = r.scope().unwrap();
        let x = read(&s, 1, "first-interned-value");
        assert_eq!(x.as_raw(), read(&s, 1, "first-interned-value").as_raw());
        assert_eq!(x.to_string().unwrap(), "first-interned-value");
        // A fixed identity (an inline or static atom) is not checked.
        let f = s.interned_string((2, 7), "fixed", false);
        assert_eq!(f.as_raw(), s.interned_string((2, 7), "fixed", false).as_raw());
        s.global().set("kept", x).unwrap();
    }
    for _ in 0..3 {
        e.collect_garbage_now();
    }
    let s = r.scope().unwrap();
    let kept = s.global().get("kept").unwrap();
    assert_eq!(read(&s, 1, "first-interned-value").as_raw(), kept.as_raw(), "a referenced string stays the realm's");
    // The identity given to another text (its host value freed and the
    // address reused) reads as that text.
    let other = read(&s, 1, "another-value-same-id");
    assert_eq!(other.to_string().unwrap(), "another-value-same-id");
    assert_eq!(read(&s, 1, "another-value-same-id").as_raw(), other.as_raw());
    assert_eq!(kept.to_string().unwrap(), "first-interned-value");
    // Another realm has its own string.
    let r2 = e.create_realm(RealmOptions::default()).unwrap();
    let s2 = r2.scope().unwrap();
    assert_eq!(read(&s2, 1, "another-value-same-id").to_string().unwrap(), "another-value-same-id");
    assert_eq!(r2.interned_string_count(), 1);
}

#[test]
fn reclaimed_strings_are_made_again_and_their_entries_swept() {
    let (e, r) = realm();
    let texts: Vec<String> = (0..20_000).map(|i| format!("unique-element-id-{i}")).collect();
    #[inline(never)]
    fn read_all(r: &Realm, texts: &[String], first: usize) {
        let s = r.scope().unwrap();
        for (i, t) in texts.iter().enumerate() {
            read(&s, (first + i) as u64, t);
        }
    }
    // Ids read once, with collections in between: the first insert after a
    // collection drops the entries of reclaimed strings, so the table keeps
    // about one batch, not 20000.
    for (b, batch) in texts.chunks(2000).enumerate() {
        read_all(&r, batch, b * 2000);
        e.collect_garbage_now();
    }
    let n = r.interned_string_count();
    assert!(n <= 4000, "{n} of 20000 entries left");
    // A reclaimed string is made again with the same text.
    let s = r.scope().unwrap();
    for (i, t) in texts[..10].iter().enumerate() {
        assert_eq!(read(&s, i as u64, t).to_string().unwrap(), *t);
    }
}

#[test]
fn dispose_releases_every_entry() {
    let (_e, r) = realm();
    {
        let s = r.scope().unwrap();
        for i in 0..100 {
            let text = format!("value-{i}");
            s.global().set(&text, read(&s, i, &text)).unwrap();
        }
    }
    assert_eq!(r.interned_string_count(), 100);
    r.dispose();
    assert_eq!(r.interned_string_count(), 0);
}
