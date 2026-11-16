//! Port of `elementhandle-misc.spec.ts`.
//!
//! All tests ported. EH-047/EH-048 read the input's value instead of
//! `window.result` (the local `input/textarea.html` only listens on the
//! textarea).

use crate::common::*;

#[test]
fn should_hover() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        let button = t.page.query_selector("#button-6").await.unwrap().unwrap();
        button.hover().await.unwrap();
        let id: String = t
            .page
            .evaluate("() => document.querySelector('button:hover').id")
            .await
            .unwrap();
        assert_eq!(id, "button-6");
    });
}

#[test]
fn should_hover_when_node_is_removed() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/scrollable.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => { delete window['Node']; }")
            .await
            .unwrap();
        let button = t.page.query_selector("#button-6").await.unwrap().unwrap();
        button.hover().await.unwrap();
        let id: String = t
            .page
            .evaluate("() => document.querySelector('button:hover').id")
            .await
            .unwrap();
        assert_eq!(id, "button-6");
    });
}

#[test]
fn should_fill_input() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        let handle = t.page.query_selector("input").await.unwrap().unwrap();
        handle.fill("some value").await.unwrap();
        let value: String = t
            .page
            .evaluate("() => document.querySelector('input').value")
            .await
            .unwrap();
        assert_eq!(value, "some value");
    });
}

#[test]
fn should_fill_input_when_node_is_removed() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/textarea.html", t.server.prefix))
            .await
            .unwrap();
        t.page
            .evaluate::<()>("() => { delete window['Node']; }")
            .await
            .unwrap();
        let handle = t.page.query_selector("input").await.unwrap().unwrap();
        handle.fill("some value").await.unwrap();
        let value: String = t
            .page
            .evaluate("() => document.querySelector('input').value")
            .await
            .unwrap();
        assert_eq!(value, "some value");
    });
}

#[test]
fn should_check_the_box() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox'></input>")
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        input.check().await.unwrap();
        assert!(t.page.evaluate::<bool>("checkbox.checked").await.unwrap());
    });
}

#[test]
fn should_check_the_box_using_set_checked() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox'></input>")
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        input.set_checked(true).await.unwrap();
        assert!(t.page.evaluate::<bool>("checkbox.checked").await.unwrap());
        input.set_checked(false).await.unwrap();
        assert!(!t.page.evaluate::<bool>("checkbox.checked").await.unwrap());
    });
}

#[test]
fn should_uncheck_the_box() {
    run(|t| async move {
        t.page
            .set_content("<input id='checkbox' type='checkbox' checked></input>")
            .await
            .unwrap();
        let input = t.page.query_selector("input").await.unwrap().unwrap();
        input.uncheck().await.unwrap();
        assert!(!t.page.evaluate::<bool>("checkbox.checked").await.unwrap());
    });
}

#[test]
fn should_select_single_option() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/select.html", t.server.prefix))
            .await
            .unwrap();
        let select = t.page.query_selector("select").await.unwrap().unwrap();
        select.select_option("blue").await.unwrap();
        let on_input: Vec<String> = t
            .page
            .evaluate("() => window['result'].onInput")
            .await
            .unwrap();
        assert_eq!(on_input, vec!["blue"]);
        let on_change: Vec<String> = t
            .page
            .evaluate("() => window['result'].onChange")
            .await
            .unwrap();
        assert_eq!(on_change, vec!["blue"]);
    });
}

#[test]
fn should_focus_a_button() {
    run(|t| async move {
        t.page
            .goto(format!("{}/input/button.html", t.server.prefix))
            .await
            .unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        assert!(
            !button
                .evaluate::<bool>("button => document.activeElement === button")
                .await
                .unwrap()
        );
        button.focus().await.unwrap();
        assert!(
            button
                .evaluate::<bool>("button => document.activeElement === button")
                .await
                .unwrap()
        );
    });
}

#[test]
fn should_allow_disposing_twice() {
    run(|t| async move {
        t.page.set_content("<section>39</section>").await.unwrap();
        let element = t.page.query_selector("section").await.unwrap().unwrap();
        element.dispose().await;
        element.dispose().await;
    });
}
