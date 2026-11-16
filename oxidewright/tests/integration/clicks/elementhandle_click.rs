//! Port of `elementhandle-click.spec.ts`.
//!
//! Skipped: none.

use serde_json::Value;

use super::eval;
use crate::common::*;

fn button_page(t: &TestContext) -> String {
    format!("{}/input/button.html", t.server.prefix)
}

#[test]
fn should_work() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        button.click().await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result']").await, "Clicked");
    });
}

#[test]
fn should_work_with_node_removed() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval(&t.page, "() => delete window['Node']").await;
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        button.click().await.unwrap();
        assert_eq!(eval(&t.page, "() => window['result']").await, "Clicked");
    });
}

#[test]
fn should_work_for_shadow_dom_v1() {
    run(|t| async move {
        t.page
            .goto(format!("{}/shadow.html", t.server.prefix))
            .await
            .unwrap();
        let handle = t
            .page
            .evaluate_handle("() => window['button']")
            .await
            .unwrap();
        handle.as_element().unwrap().click().await.unwrap();
        assert_eq!(eval(&t.page, "clicked").await, true);
    });
}

#[test]
fn should_work_for_text_nodes() {
    run(|t| async move {
        t.page
            .set_content(
                r#"<div id='outer' onclick="window['result'] = 'Clicked ' + event.target.id;">
                  <div id='inner' style="max-width: 50px">Lorem ipsum dolor sit amet consectetur adipiscing elit proin, integer curabitur imperdiet rhoncus cursus tincidunt bibendum, consequat sed magnis laoreet luctus mollis tellus. Nisl parturient mus accumsan feugiat sem laoreet magnis nisi, aptent per sollicitudin gravida orci ac blandit, viverra eros praesent auctor vivamus semper bibendum. Consequat sed habitasse luctus dictumst gravida platea semper phasellus, nascetur ridiculus purus est varius quisque et scelerisque, id vehicula eleifend montes sollicitudin dis velit. Pellentesque ridiculus per natoque et eleifend taciti nunc, laoreet auctor at condimentum imperdiet ante, conubia mi cubilia scelerisque sociosqu sem.</div>
                  Custom Text.
                </div>"#,
            )
            .await
            .unwrap();
        let text_node = t
            .page
            .evaluate_handle("() => document.querySelector('#outer').lastChild")
            .await
            .unwrap();
        text_node.as_element().unwrap().click().await.unwrap();
        assert_eq!(
            eval(&t.page, "() => window['result']").await,
            "Clicked outer"
        );
    });
}

#[test]
fn should_throw_for_detached_nodes() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        t.page
            .evaluate::<Value>("button => button.remove()")
            .arg(&button)
            .await
            .unwrap();
        assert_err_contains(button.click().await, "Element is not attached to the DOM");
    });
}

#[test]
fn should_throw_for_hidden_nodes_with_force() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        t.page
            .evaluate::<Value>("button => button.style.display = 'none'")
            .arg(&button)
            .await
            .unwrap();
        assert_err_contains(button.click().force(true).await, "Element is not visible");
    });
}

#[test]
fn should_throw_for_recursively_hidden_nodes_with_force() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        t.page
            .evaluate::<Value>("button => button.parentElement.style.display = 'none'")
            .arg(&button)
            .await
            .unwrap();
        assert_err_contains(button.click().force(true).await, "Element is not visible");
    });
}

#[test]
fn should_throw_for_br_elements_with_force() {
    run(|t| async move {
        t.page.set_content("hello<br>goodbye").await.unwrap();
        let br = t.page.query_selector("br").await.unwrap().unwrap();
        assert_err_contains(
            br.click().force(true).await,
            "Element is outside of the viewport",
        );
    });
}

#[test]
fn should_double_click_the_button() {
    run(|t| async move {
        t.page.goto(button_page(&t)).await.unwrap();
        eval(
            &t.page,
            "() => { window['double'] = false; const button = document.querySelector('button'); button.addEventListener('dblclick', event => { window['double'] = true; }); }",
        )
        .await;
        let button = t.page.query_selector("button").await.unwrap().unwrap();
        button.dblclick().await.unwrap();
        assert_eq!(eval(&t.page, "double").await, true);
        assert_eq!(eval(&t.page, "result").await, "Clicked");
    });
}
