use crate::common::*;
use oxidewright::LoadState;

#[test]
fn smoke_goto_and_evaluate() {
    run(|t| async move {
        let response = t.page.goto(&t.server.empty_page).await.unwrap().unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(t.page.url(), t.server.empty_page);
        let v: i32 = t.page.evaluate("() => 7 * 3").await.unwrap();
        assert_eq!(v, 21);
        let v: i32 = t.page.evaluate("a => a * 2").arg(4).await.unwrap();
        assert_eq!(v, 8);
        t.page
            .goto(format!("{}/title.html", t.server.prefix))
            .wait_until(LoadState::NetworkIdle)
            .await
            .unwrap();
        assert_eq!(t.page.title().await.unwrap(), "Woof-Woof");
    });
}
