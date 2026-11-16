//! Port of `locator-evaluate.spec.ts`. All tests ported.

use crate::common::*;

#[test]
fn should_work() {
    run(|t| async move {
        t.page
            .set_content("<html><body><div class=\"tweet\"><div class=\"like\">100</div><div class=\"retweets\">10</div></div></body></html>")
            .await
            .unwrap();
        let tweet = t.page.locator(".tweet .like");
        let content: String = tweet.evaluate("node => node.innerText").await.unwrap();
        assert_eq!(content, "100");
    });
}

#[test]
fn should_retrieve_content_from_subtree() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"a\">not-a-child-div</div><div id=\"myId\"><div class=\"a\">a-child-div</div></div>")
            .await
            .unwrap();
        let element = t.page.locator("#myId .a");
        let content: String = element.evaluate("node => node.innerText").await.unwrap();
        assert_eq!(content, "a-child-div");
    });
}

#[test]
fn should_work_for_all() {
    run(|t| async move {
        t.page
            .set_content("<html><body><div class=\"tweet\"><div class=\"like\">100</div><div class=\"like\">10</div></div></body></html>")
            .await
            .unwrap();
        let tweet = t.page.locator(".tweet .like");
        let content: Vec<String> = tweet
            .evaluate_all("nodes => nodes.map(n => n.innerText)")
            .await
            .unwrap();
        assert_eq!(content, vec!["100", "10"]);
    });
}

#[test]
fn should_retrieve_content_from_subtree_for_all() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"a\">not-a-child-div</div><div id=\"myId\"><div class=\"a\">a1-child-div</div><div class=\"a\">a2-child-div</div></div>")
            .await
            .unwrap();
        let element = t.page.locator("#myId .a");
        let content: Vec<String> = element
            .evaluate_all("nodes => nodes.map(n => n.innerText)")
            .await
            .unwrap();
        assert_eq!(content, vec!["a1-child-div", "a2-child-div"]);
    });
}

#[test]
fn should_not_throw_in_case_of_missing_selector_for_all() {
    run(|t| async move {
        t.page
            .set_content("<div class=\"a\">not-a-child-div</div><div id=\"myId\"></div>")
            .await
            .unwrap();
        let element = t.page.locator("#myId .a");
        let length: usize = element.evaluate_all("nodes => nodes.length").await.unwrap();
        assert_eq!(length, 0);
    });
}
