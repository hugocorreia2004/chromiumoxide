//! [`Locator`] and [`FrameLocator`], a port of Playwright's
//! `client/locator.ts`. Locators are lazy: they only hold a frame and a
//! selector string (built exactly like Playwright builds it) and resolve it
//! on every action, with strict mode on.

use std::fmt;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::actions::{
    Action, Click, Dblclick, Fill, Hover, InputFiles, Press, SelectOptionAction, SelectValues,
    SetChecked, SetInputFiles, Tap, Target, Type, kinds,
};
use crate::element_actions::WaitForSelectorState;
use crate::error::{Error, Result, ResultExt};
use crate::frame::Frame;
use crate::frame_selectors::{
    SelectorTarget, call_injected, query, query_all, query_count, value_of,
};
use crate::js_handle::ElementHandle;
use crate::locator_generators::{as_locator_description, locator_custom_description};
use crate::locator_utils::{
    ByRoleOptions, TextMatch, escape_for_text_selector, get_by_alt_text_selector,
    get_by_label_selector, get_by_placeholder_selector, get_by_role_selector,
    get_by_test_id_selector, get_by_text_selector, get_by_title_selector, json_string,
};
use crate::page::Page;
use crate::selector_parser::ANY_FRAME_SELECTOR;
use crate::selectors::test_id_attribute_name;

/// Options of `locator(...)` / `filter(...)`: `has_text`, `has_not_text`,
/// `has`, `has_not` and `visible`.
#[derive(Debug, Clone, Default)]
pub struct LocatorOptions {
    has_text: Option<TextMatch>,
    has_not_text: Option<TextMatch>,
    has: Option<Locator>,
    has_not: Option<Locator>,
    visible: Option<bool>,
}

impl LocatorOptions {
    /// No filter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Matches elements containing `text` somewhere inside (case-insensitive
    /// substring for strings), possibly in a child or descendant.
    pub fn has_text(mut self, text: impl Into<TextMatch>) -> Self {
        self.has_text = Some(text.into());
        self
    }

    /// Matches elements not containing `text`.
    pub fn has_not_text(mut self, text: impl Into<TextMatch>) -> Self {
        self.has_not_text = Some(text.into());
        self
    }

    /// Matches elements containing an element matched by `locator`.
    pub fn has(mut self, locator: &Locator) -> Self {
        self.has = Some(locator.clone());
        self
    }

    /// Matches elements not containing an element matched by `locator`.
    pub fn has_not(mut self, locator: &Locator) -> Self {
        self.has_not = Some(locator.clone());
        self
    }

    /// Only visible (`true`) or hidden (`false`) elements.
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = Some(visible);
        self
    }
}

/// Options of `get_by_role`.
#[derive(Debug, Clone, Default)]
pub struct GetByRoleOptions {
    inner: ByRoleOptions,
}

impl GetByRoleOptions {
    /// No option.
    pub fn new() -> Self {
        Self::default()
    }
    /// `aria-checked` / native checked state.
    pub fn checked(mut self, checked: bool) -> Self {
        self.inner.checked = Some(checked);
        self
    }
    /// `aria-disabled` / native disabled state.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.inner.disabled = Some(disabled);
        self
    }
    /// Match the accessible name exactly (case-sensitive, whole string).
    pub fn exact(mut self, exact: bool) -> Self {
        self.inner.exact = Some(exact);
        self
    }
    /// `aria-expanded`.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.inner.expanded = Some(expanded);
        self
    }
    /// Also match hidden elements.
    pub fn include_hidden(mut self, include_hidden: bool) -> Self {
        self.inner.include_hidden = Some(include_hidden);
        self
    }
    /// Heading level (`aria-level`).
    pub fn level(mut self, level: i64) -> Self {
        self.inner.level = Some(level);
        self
    }
    /// Accessible name (substring, case-insensitive unless `exact`; or a regex).
    pub fn name(mut self, name: impl Into<TextMatch>) -> Self {
        self.inner.name = Some(name.into());
        self
    }
    /// Accessible description.
    pub fn description(mut self, description: impl Into<TextMatch>) -> Self {
        self.inner.description = Some(description.into());
        self
    }
    /// `aria-pressed`.
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.inner.pressed = Some(pressed);
        self
    }
    /// `aria-selected`.
    pub fn selected(mut self, selected: bool) -> Self {
        self.inner.selected = Some(selected);
        self
    }
}

/// Options of `get_by_text`, `get_by_label`, ... (`exact`).
#[derive(Debug, Clone, Copy, Default)]
pub struct GetByOptions {
    /// Whole-string, case-sensitive match.
    pub exact: bool,
}

impl GetByOptions {
    /// `{ exact: true }`.
    pub fn exact() -> Self {
        GetByOptions { exact: true }
    }
}

/// What `locator(...)` accepts: a selector or another locator.
#[derive(Debug, Clone)]
pub enum SelectorOrLocator {
    /// A selector string.
    Selector(String),
    /// A locator of the same frame.
    Locator(Locator),
}

impl From<&str> for SelectorOrLocator {
    fn from(v: &str) -> Self {
        SelectorOrLocator::Selector(v.to_string())
    }
}
impl From<String> for SelectorOrLocator {
    fn from(v: String) -> Self {
        SelectorOrLocator::Selector(v)
    }
}
impl From<&String> for SelectorOrLocator {
    fn from(v: &String) -> Self {
        SelectorOrLocator::Selector(v.clone())
    }
}
impl From<Locator> for SelectorOrLocator {
    fn from(v: Locator) -> Self {
        SelectorOrLocator::Locator(v)
    }
}
impl From<&Locator> for SelectorOrLocator {
    fn from(v: &Locator) -> Self {
        SelectorOrLocator::Locator(v.clone())
    }
}

/// A way to find element(s) on the page at any moment. Cheap to clone.
#[derive(Clone)]
pub struct Locator {
    frame: Frame,
    selector: String,
    /// A construction error (Playwright throws synchronously).
    error: Option<String>,
}

impl PartialEq for Locator {
    fn eq(&self, other: &Self) -> bool {
        self.frame == other.frame && self.selector == other.selector
    }
}

impl fmt::Debug for Locator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Locator")
            .field("selector", &self.selector)
            .finish()
    }
}

impl fmt::Display for Locator {
    /// `locator.toString()`: the locator in JavaScript syntax, e.g.
    /// `getByRole('button', { name: 'Submit' })`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&as_locator_description(&self.selector))
    }
}

impl Locator {
    pub(crate) fn new(frame: Frame, selector: String, options: Option<&LocatorOptions>) -> Self {
        let mut locator = Locator {
            frame,
            selector,
            error: None,
        };
        if let Some(options) = options {
            locator.apply_options(options);
        }
        locator
    }

    fn apply_options(&mut self, options: &LocatorOptions) {
        if let Some(text) = &options.has_text {
            self.selector += &format!(
                " >> internal:has-text={}",
                escape_for_text_selector(text, false)
            );
        }
        if let Some(text) = &options.has_not_text {
            self.selector += &format!(
                " >> internal:has-not-text={}",
                escape_for_text_selector(text, false)
            );
        }
        if let Some(has) = &options.has {
            if has.frame != self.frame {
                self.fail("Inner \"has\" locator must belong to the same frame.");
            }
            self.inherit_error(has);
            self.selector += &format!(" >> internal:has={}", json_string(&has.selector));
        }
        if let Some(has_not) = &options.has_not {
            if has_not.frame != self.frame {
                self.fail("Inner \"hasNot\" locator must belong to the same frame.");
            }
            self.inherit_error(has_not);
            self.selector += &format!(" >> internal:has-not={}", json_string(&has_not.selector));
        }
        if let Some(visible) = options.visible {
            self.selector += &format!(" >> visible={visible}");
        }
    }

    fn fail(&mut self, message: &str) {
        if self.error.is_none() {
            self.error = Some(message.to_string());
        }
    }

    fn inherit_error(&mut self, other: &Locator) {
        if let Some(e) = &other.error {
            self.fail(e);
        }
    }

    fn derive(&self, selector: String) -> Locator {
        Locator {
            frame: self.frame.clone(),
            selector,
            error: self.error.clone(),
        }
    }

    /// The selector this locator resolves (Playwright's internal `_selector`).
    pub fn selector(&self) -> &str {
        &self.selector
    }

    /// The error of an invalid locator (Playwright throws it when the
    /// locator is built).
    pub(crate) fn construction_error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The frame this locator belongs to.
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// The page this locator belongs to.
    pub fn page(&self) -> Option<Page> {
        self.frame.page()
    }

    fn target(&self) -> Target {
        match &self.error {
            Some(e) => Target::Invalid(e.clone()),
            None => Target::selector(&self.frame, self.selector.clone(), true),
        }
    }

    fn action<K>(&self, api: &'static str) -> Action<K> {
        Action::new(self.target(), api)
    }

    pub(crate) fn action_for<K>(&self, api: &'static str) -> Action<K> {
        self.action(api)
    }

    fn check_error(&self) -> Result<()> {
        match &self.error {
            Some(e) => Err(Error::invalid_argument(e.clone())),
            None => Ok(()),
        }
    }

    // ------------------------------------------------------------------
    // Building locators
    // ------------------------------------------------------------------

    /// A locator for `selector_or_locator` inside this locator.
    pub fn locator(&self, selector_or_locator: impl Into<SelectorOrLocator>) -> Locator {
        self.locator_with(selector_or_locator, LocatorOptions::default())
    }

    /// Like [`Locator::locator`] with filter options.
    pub fn locator_with(
        &self,
        selector_or_locator: impl Into<SelectorOrLocator>,
        options: LocatorOptions,
    ) -> Locator {
        let options = LocatorOptions {
            visible: None,
            ..options
        };
        match selector_or_locator.into() {
            SelectorOrLocator::Selector(s) => {
                let mut l = self.derive(format!("{} >> {s}", self.selector));
                l.apply_options(&options);
                l
            }
            SelectorOrLocator::Locator(other) => {
                let mut l = self.derive(format!(
                    "{} >> internal:chain={}",
                    self.selector,
                    json_string(&other.selector)
                ));
                if other.frame != self.frame {
                    l.fail("Locators must belong to the same frame.");
                }
                l.inherit_error(&other);
                l.apply_options(&options);
                l
            }
        }
    }

    /// This locator scoped inside `locator` (`locator.locator(this)`).
    pub fn within(&self, locator: &Locator) -> Locator {
        locator.locator(self)
    }

    /// Elements by test id attribute (see [`crate::Selectors::set_test_id_attribute`]).
    pub fn get_by_test_id(&self, test_id: impl Into<TextMatch>) -> Locator {
        self.locator(get_by_test_id_selector(
            &test_id_attribute_name(),
            &test_id.into(),
        ))
    }

    /// Elements by `alt` text.
    pub fn get_by_alt_text(&self, text: impl Into<TextMatch>) -> Locator {
        self.get_by_alt_text_with(text, GetByOptions::default())
    }

    /// Elements by `alt` text, with options.
    pub fn get_by_alt_text_with(
        &self,
        text: impl Into<TextMatch>,
        options: GetByOptions,
    ) -> Locator {
        self.locator(get_by_alt_text_selector(&text.into(), options.exact))
    }

    /// Inputs by label text (or `aria-labelledby`/`aria-label`).
    pub fn get_by_label(&self, text: impl Into<TextMatch>) -> Locator {
        self.get_by_label_with(text, GetByOptions::default())
    }

    /// Inputs by label text, with options.
    pub fn get_by_label_with(&self, text: impl Into<TextMatch>, options: GetByOptions) -> Locator {
        self.locator(get_by_label_selector(&text.into(), options.exact))
    }

    /// Inputs by placeholder.
    pub fn get_by_placeholder(&self, text: impl Into<TextMatch>) -> Locator {
        self.get_by_placeholder_with(text, GetByOptions::default())
    }

    /// Inputs by placeholder, with options.
    pub fn get_by_placeholder_with(
        &self,
        text: impl Into<TextMatch>,
        options: GetByOptions,
    ) -> Locator {
        self.locator(get_by_placeholder_selector(&text.into(), options.exact))
    }

    /// Elements by text content.
    pub fn get_by_text(&self, text: impl Into<TextMatch>) -> Locator {
        self.get_by_text_with(text, GetByOptions::default())
    }

    /// Elements by text content, with options.
    pub fn get_by_text_with(&self, text: impl Into<TextMatch>, options: GetByOptions) -> Locator {
        self.locator(get_by_text_selector(&text.into(), options.exact))
    }

    /// Elements by `title` attribute.
    pub fn get_by_title(&self, text: impl Into<TextMatch>) -> Locator {
        self.get_by_title_with(text, GetByOptions::default())
    }

    /// Elements by `title` attribute, with options.
    pub fn get_by_title_with(&self, text: impl Into<TextMatch>, options: GetByOptions) -> Locator {
        self.locator(get_by_title_selector(&text.into(), options.exact))
    }

    /// Elements by ARIA role.
    pub fn get_by_role(&self, role: &str) -> Locator {
        self.get_by_role_with(role, GetByRoleOptions::default())
    }

    /// Elements by ARIA role, with options.
    pub fn get_by_role_with(&self, role: &str, options: GetByRoleOptions) -> Locator {
        self.locator(get_by_role_selector(role, &options.inner))
    }

    /// A frame locator for the `<iframe>` matching `selector` inside this locator.
    pub fn frame_locator(&self, selector: &str) -> FrameLocator {
        FrameLocator {
            frame: self.frame.clone(),
            selector: format!("{} >> {selector}", self.selector),
            error: self.error.clone(),
        }
    }

    /// A frame locator for the `<iframe>` this locator points to.
    pub fn content_frame(&self) -> FrameLocator {
        FrameLocator {
            frame: self.frame.clone(),
            selector: self.selector.clone(),
            error: self.error.clone(),
        }
    }

    /// Narrow this locator down with filter options.
    pub fn filter(&self, options: LocatorOptions) -> Locator {
        let mut l = self.derive(self.selector.clone());
        l.apply_options(&options);
        l
    }

    /// Only visible elements (`filter({ visible: true })`).
    pub fn visible(&self) -> Locator {
        self.filter(LocatorOptions::default().visible(true))
    }

    /// The first matching element.
    pub fn first(&self) -> Locator {
        self.derive(format!("{} >> nth=0", self.selector))
    }

    /// The last matching element.
    pub fn last(&self) -> Locator {
        self.derive(format!("{} >> nth=-1", self.selector))
    }

    /// The `index`-th (zero based, negative from the end) matching element.
    pub fn nth(&self, index: i64) -> Locator {
        self.derive(format!("{} >> nth={index}", self.selector))
    }

    /// Elements matching both this locator and `locator`.
    pub fn and_(&self, locator: &Locator) -> Locator {
        let mut l = self.derive(format!(
            "{} >> internal:and={}",
            self.selector,
            json_string(&locator.selector)
        ));
        if locator.frame != self.frame {
            l.fail("Locators must belong to the same frame.");
        }
        l.inherit_error(locator);
        l
    }

    /// Elements matching this locator or `locator`.
    pub fn or_(&self, locator: &Locator) -> Locator {
        let mut l = self.derive(format!(
            "{} >> internal:or={}",
            self.selector,
            json_string(&locator.selector)
        ));
        if locator.frame != self.frame {
            l.fail("Locators must belong to the same frame.");
        }
        l.inherit_error(locator);
        l
    }

    /// This locator with a human readable description (used in logs/errors).
    pub fn describe(&self, description: &str) -> Locator {
        self.derive(format!(
            "{} >> internal:describe={}",
            self.selector,
            json_string(description)
        ))
    }

    /// The description given with [`Locator::describe`].
    pub fn description(&self) -> Option<String> {
        locator_custom_description(&self.selector)
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Number of matching elements.
    pub async fn count(&self) -> Result<usize> {
        async {
            self.check_error()?;
            query_count(&self.frame, &self.selector).await
        }
        .await
        .api("locator.count")
    }

    /// A locator for each matching element (`nth(0)`, `nth(1)`, ...).
    pub async fn all(&self) -> Result<Vec<Locator>> {
        let count = self.count().await.api("locator.all")?;
        Ok((0..count).map(|i| self.nth(i as i64)).collect())
    }

    /// `innerText` of every matching element.
    pub async fn all_inner_texts(&self) -> Result<Vec<String>> {
        self.evaluate_all::<Vec<String>>("ee => ee.map(e => e.innerText)")
            .await
    }

    /// `textContent` of every matching element.
    pub async fn all_text_contents(&self) -> Result<Vec<String>> {
        self.evaluate_all::<Vec<String>>("ee => ee.map(e => e.textContent || '')")
            .await
    }

    /// Resolve to the (single) matching element handle.
    pub fn element_handle(&self) -> Action<kinds::ElementHandle> {
        self.action("locator.elementHandle")
    }

    /// [`Self::element_handle`] reporting errors as `api`.
    pub(crate) fn element_handle_as(&self, api: &'static str) -> Action<kinds::ElementHandle> {
        self.action(api)
    }

    /// Handles to all matching elements.
    pub async fn element_handles(&self) -> Result<Vec<ElementHandle>> {
        async {
            self.check_error()?;
            query_all(&self.frame, &self.selector, None).await
        }
        .await
        .api("locator.elementHandles")
    }

    /// Evaluate `expression` with the matching element as argument.
    pub fn evaluate<T: DeserializeOwned>(
        &self,
        expression: impl Into<String>,
    ) -> Action<kinds::Evaluate<T>> {
        self.action("locator.evaluate").with_text(expression)
    }

    /// Evaluate `expression` with all matching elements as argument.
    pub fn evaluate_all<T: DeserializeOwned>(
        &self,
        expression: impl Into<String>,
    ) -> Action<kinds::EvaluateAll<T>> {
        self.action("locator.evaluateAll").with_text(expression)
    }

    /// Like [`Locator::evaluate`], returning a handle.
    pub fn evaluate_handle(&self, expression: impl Into<String>) -> Action<kinds::EvaluateHandle> {
        self.action("locator.evaluateHandle").with_text(expression)
    }

    /// The bounding box of the matching element.
    pub fn bounding_box(&self) -> Action<kinds::BoundingBox> {
        self.action("locator.boundingBox")
    }

    /// `textContent` of the matching element.
    pub fn text_content(&self) -> Action<kinds::TextContent> {
        self.action("locator.textContent")
    }

    /// `innerText` of the matching element.
    pub fn inner_text(&self) -> Action<kinds::InnerText> {
        self.action("locator.innerText")
    }

    /// `innerHTML` of the matching element.
    pub fn inner_html(&self) -> Action<kinds::InnerHtml> {
        self.action("locator.innerHTML")
    }

    /// An attribute of the matching element.
    pub fn get_attribute(&self, name: impl Into<String>) -> Action<kinds::GetAttribute> {
        self.action("locator.getAttribute").with_text(name)
    }

    /// `value` of the matching input.
    pub fn input_value(&self) -> Action<kinds::InputValue> {
        self.action("locator.inputValue")
    }

    /// Whether the matching element is checked.
    pub fn is_checked(&self) -> Action<kinds::IsState> {
        self.action("locator.isChecked").with_text("checked")
    }

    /// Whether the matching element is disabled.
    pub fn is_disabled(&self) -> Action<kinds::IsState> {
        self.action("locator.isDisabled").with_text("disabled")
    }

    /// Whether the matching element is editable.
    pub fn is_editable(&self) -> Action<kinds::IsState> {
        self.action("locator.isEditable").with_text("editable")
    }

    /// Whether the matching element is enabled.
    pub fn is_enabled(&self) -> Action<kinds::IsState> {
        self.action("locator.isEnabled").with_text("enabled")
    }

    /// Whether the matching element is hidden (does not wait).
    pub fn is_hidden(&self) -> Action<kinds::IsVisible> {
        self.action("locator.isHidden").with_flag(true)
    }

    /// Whether the matching element is visible (does not wait).
    pub fn is_visible(&self) -> Action<kinds::IsVisible> {
        self.action("locator.isVisible")
    }

    /// Wait until the locator reaches a state (default visible).
    pub fn wait_for(&self) -> Action<kinds::WaitFor> {
        self.action("locator.waitFor")
    }

    /// The ARIA snapshot (YAML) of the matching element.
    pub fn aria_snapshot(&self) -> Action<kinds::AriaSnapshot> {
        self.action("locator.ariaSnapshot")
    }

    // ------------------------------------------------------------------
    // Actions
    // ------------------------------------------------------------------

    /// Click the element.
    pub fn click(&self) -> Click {
        self.action("locator.click")
    }

    /// Double click the element.
    pub fn dblclick(&self) -> Dblclick {
        self.action("locator.dblclick")
    }

    /// Tap the element.
    pub fn tap(&self) -> Tap {
        self.action("locator.tap")
    }

    /// Hover over the element.
    pub fn hover(&self) -> Hover {
        self.action("locator.hover")
    }

    /// Fill the input with `value`.
    pub fn fill(&self, value: impl Into<String>) -> Fill {
        self.action("locator.fill").with_text(value)
    }

    /// Clear the input (`fill("")`).
    pub fn clear(&self) -> Fill {
        self.action("locator.clear").with_text("")
    }

    /// Focus the element and press `key`.
    pub fn press(&self, key: impl Into<String>) -> Press {
        self.action("locator.press").with_text(key)
    }

    /// Focus the element and type `text` key by key.
    pub fn press_sequentially(&self, text: impl Into<String>) -> Type {
        self.action("locator.pressSequentially").with_text(text)
    }

    /// Deprecated alias of [`Locator::press_sequentially`].
    pub fn r#type(&self, text: impl Into<String>) -> Type {
        self.action("locator.type").with_text(text)
    }

    /// Check a checkbox or radio.
    pub fn check(&self) -> SetChecked {
        self.action("locator.check").with_flag(true)
    }

    /// Uncheck a checkbox.
    pub fn uncheck(&self) -> SetChecked {
        self.action("locator.uncheck").with_flag(false)
    }

    /// Check or uncheck.
    pub fn set_checked(&self, checked: bool) -> SetChecked {
        self.action("locator.setChecked").with_flag(checked)
    }

    /// Select options of a `<select>`.
    pub fn select_option(&self, values: impl Into<SelectValues>) -> SelectOptionAction {
        let mut action: SelectOptionAction = self.action("locator.selectOption");
        action.args.values = values.into();
        action
    }

    /// Select the text of the element.
    pub fn select_text(&self) -> Action<kinds::SelectText> {
        self.action("locator.selectText")
    }

    /// Set the files of a file input.
    pub fn set_input_files(&self, files: impl Into<InputFiles>) -> SetInputFiles {
        let mut action: SetInputFiles = self.action("locator.setInputFiles");
        action.args.files = Some(files.into());
        action
    }

    /// Focus the element.
    pub fn focus(&self) -> Action<kinds::Focus> {
        self.action("locator.focus")
    }

    /// Remove focus from the element.
    pub fn blur(&self) -> Action<kinds::Blur> {
        self.action("locator.blur")
    }

    /// Dispatch a DOM event on the element.
    pub fn dispatch_event(&self, type_: impl Into<String>) -> Action<kinds::DispatchEvent> {
        self.action("locator.dispatchEvent").with_text(type_)
    }

    /// Scroll the element into view if needed.
    pub fn scroll_into_view_if_needed(&self) -> Action<kinds::ScrollIntoView> {
        self.action("locator.scrollIntoViewIfNeeded")
    }

    /// Drag this element onto `target`.
    pub fn drag_to(&self, target: &Locator) -> Action<kinds::DragAndDrop> {
        let mut action: Action<kinds::DragAndDrop> = self.action("locator.dragTo");
        action.args.other = Some(target.selector.clone());
        action
    }

    /// Highlight the matching elements on screen (debugging aid). The
    /// highlight follows the elements (it is resolved again every second
    /// and after navigations) until hidden. Builder: .style(css), then
    /// .await it.
    pub fn highlight(&self) -> Highlight {
        Highlight {
            locator: self.clone(),
            style: None,
        }
    }

    /// Remove the highlight added by [Locator::highlight].
    pub async fn hide_highlight(&self) -> Result<()> {
        async {
            self.check_error()?;
            let page = self.frame.page_inner().ok_or_else(Error::target_closed)?;
            crate::page_state::remove_highlight(&page, &self.frame, &self.selector).await;
            Ok(())
        }
        .await
        .api("locator.hideHighlight")
    }

    /// The ARIA snapshot of the matching element as a JSON tree.
    pub fn aria_snapshot_json(&self) -> Action<kinds::AriaSnapshotJson> {
        self.action("locator.ariaSnapshotJSON")
    }
    /// A locator with an equivalent, normalized selector (`locator.normalize()`).
    pub async fn normalize(&self) -> Result<Locator> {
        async {
            self.check_error()?;
            let target = SelectorTarget::new(self.selector.clone(), false);
            let element = query(&self.frame, &target).await?.ok_or_else(|| {
                Error::evaluation(format!("No element matching {}", self.selector))
            })?;
            let mut parts = Vec::new();
            let mut current = Some(element);
            while let Some(element) = current.take() {
                let generated = element
                    .eval_in_utility(
                        "(injected, node) => injected.generateSelectorSimple(node)",
                        Value::Null,
                    )
                    .await;
                let frame = element.owner_frame_sync();
                element.dispose().await;
                let generated = generated?
                    .and_then(|v| v.as_str().map(str::to_string))
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        Error::evaluation(format!(
                            "Unable to generate locator for {}",
                            self.selector
                        ))
                    })?;
                parts.push(generated);
                let frame = frame?;
                if frame.parent_frame().is_some() {
                    current = Some(frame.frame_element().await?);
                }
            }
            parts.reverse();
            Ok(self.derive(parts.join(" >> internal:control=enter-frame >> ")))
        }
        .await
        .api("locator.normalize")
    }
}

/// Builder returned by [Locator::highlight]; .await it.
#[must_use = "builders do nothing unless awaited"]
pub struct Highlight {
    locator: Locator,
    style: Option<String>,
}

impl Highlight {
    /// Extra CSS applied to the highlight box, e.g.
    /// "outline: 2px solid red".
    pub fn style(mut self, css: impl Into<String>) -> Self {
        self.style = Some(css.into());
        self
    }
}

impl std::future::IntoFuture for Highlight {
    type Output = Result<()>;
    type IntoFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>;
    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            let locator = self.locator;
            async {
                locator.check_error()?;
                let page = locator
                    .frame
                    .page_inner()
                    .ok_or_else(Error::target_closed)?;
                crate::page_state::add_highlight(
                    &page,
                    &locator.frame,
                    &locator.selector,
                    self.style,
                )
                .await
            }
            .await
            .api("locator.highlight")
        })
    }
}

/// A locator for the content of an `<iframe>`.
#[derive(Clone)]
pub struct FrameLocator {
    frame: Frame,
    selector: String,
    error: Option<String>,
}

impl fmt::Debug for FrameLocator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameLocator")
            .field("selector", &self.selector)
            .finish()
    }
}

impl FrameLocator {
    fn child_selector(&self, selector: &str) -> String {
        if self.selector == ANY_FRAME_SELECTOR {
            format!("{} >> {selector}", self.selector)
        } else {
            format!(
                "{} >> internal:control=enter-frame >> {selector}",
                self.selector
            )
        }
    }

    fn child(&self, selector: String) -> Locator {
        Locator {
            frame: self.frame.clone(),
            selector,
            error: self.error.clone(),
        }
    }

    /// A locator inside the frame.
    pub fn locator(&self, selector_or_locator: impl Into<SelectorOrLocator>) -> Locator {
        self.locator_with(selector_or_locator, LocatorOptions::default())
    }

    /// A locator inside the frame, with filter options.
    pub fn locator_with(
        &self,
        selector_or_locator: impl Into<SelectorOrLocator>,
        options: LocatorOptions,
    ) -> Locator {
        match selector_or_locator.into() {
            SelectorOrLocator::Selector(s) => {
                let mut l = self.child(self.child_selector(&s));
                l.apply_options(&options);
                l
            }
            SelectorOrLocator::Locator(other) => {
                let mut l = self.child(self.child_selector(&other.selector));
                if other.frame != self.frame {
                    l.fail("Locators must belong to the same frame.");
                }
                l.inherit_error(&other);
                l.apply_options(&options);
                l
            }
        }
    }

    /// Elements by test id inside the frame.
    pub fn get_by_test_id(&self, test_id: impl Into<TextMatch>) -> Locator {
        self.locator(get_by_test_id_selector(
            &test_id_attribute_name(),
            &test_id.into(),
        ))
    }

    /// Elements by `alt` text inside the frame.
    pub fn get_by_alt_text(&self, text: impl Into<TextMatch>) -> Locator {
        self.locator(get_by_alt_text_selector(&text.into(), false))
    }

    /// Elements by `alt` text inside the frame, with options.
    pub fn get_by_alt_text_with(
        &self,
        text: impl Into<TextMatch>,
        options: GetByOptions,
    ) -> Locator {
        self.locator(get_by_alt_text_selector(&text.into(), options.exact))
    }

    /// Inputs by label inside the frame.
    pub fn get_by_label(&self, text: impl Into<TextMatch>) -> Locator {
        self.locator(get_by_label_selector(&text.into(), false))
    }

    /// Inputs by label inside the frame, with options.
    pub fn get_by_label_with(&self, text: impl Into<TextMatch>, options: GetByOptions) -> Locator {
        self.locator(get_by_label_selector(&text.into(), options.exact))
    }

    /// Inputs by placeholder inside the frame.
    pub fn get_by_placeholder(&self, text: impl Into<TextMatch>) -> Locator {
        self.locator(get_by_placeholder_selector(&text.into(), false))
    }

    /// Inputs by placeholder inside the frame, with options.
    pub fn get_by_placeholder_with(
        &self,
        text: impl Into<TextMatch>,
        options: GetByOptions,
    ) -> Locator {
        self.locator(get_by_placeholder_selector(&text.into(), options.exact))
    }

    /// Elements by text inside the frame.
    pub fn get_by_text(&self, text: impl Into<TextMatch>) -> Locator {
        self.locator(get_by_text_selector(&text.into(), false))
    }

    /// Elements by text inside the frame, with options.
    pub fn get_by_text_with(&self, text: impl Into<TextMatch>, options: GetByOptions) -> Locator {
        self.locator(get_by_text_selector(&text.into(), options.exact))
    }

    /// Elements by `title` inside the frame.
    pub fn get_by_title(&self, text: impl Into<TextMatch>) -> Locator {
        self.locator(get_by_title_selector(&text.into(), false))
    }

    /// Elements by `title` inside the frame, with options.
    pub fn get_by_title_with(&self, text: impl Into<TextMatch>, options: GetByOptions) -> Locator {
        self.locator(get_by_title_selector(&text.into(), options.exact))
    }

    /// Elements by ARIA role inside the frame.
    pub fn get_by_role(&self, role: &str) -> Locator {
        self.get_by_role_with(role, GetByRoleOptions::default())
    }

    /// Elements by ARIA role inside the frame, with options.
    pub fn get_by_role_with(&self, role: &str, options: GetByRoleOptions) -> Locator {
        self.locator(get_by_role_selector(role, &options.inner))
    }

    /// A locator for the `<iframe>` element itself.
    pub fn owner(&self) -> Locator {
        self.child(self.selector.clone())
    }

    /// A frame locator for an `<iframe>` inside this frame.
    pub fn frame_locator(&self, selector: &str) -> FrameLocator {
        FrameLocator {
            frame: self.frame.clone(),
            selector: self.child_selector(selector),
            error: self.error.clone(),
        }
    }

    fn nth_selector(&self, nth: &str) -> FrameLocator {
        let mut error = self.error.clone();
        if self.selector == ANY_FRAME_SELECTOR && error.is_none() {
            error = Some("Selecting the nth frame is not allowed on frameLocator().".to_string());
        }
        FrameLocator {
            frame: self.frame.clone(),
            selector: format!("{} >> nth={nth}", self.selector),
            error,
        }
    }

    /// The first matching frame.
    pub fn first(&self) -> FrameLocator {
        self.nth_selector("0")
    }

    /// The last matching frame.
    pub fn last(&self) -> FrameLocator {
        self.nth_selector("-1")
    }

    /// The `index`-th matching frame.
    pub fn nth(&self, index: i64) -> FrameLocator {
        self.nth_selector(&index.to_string())
    }
}

macro_rules! locator_api {
    ($ty:ty, $frame:ident => $get_frame:expr) => {
        impl $ty {
            /// A locator for `selector_or_locator` in this frame.
            pub fn locator(&self, selector_or_locator: impl Into<SelectorOrLocator>) -> Locator {
                self.locator_with(selector_or_locator, LocatorOptions::default())
            }

            /// A locator with filter options.
            pub fn locator_with(
                &self,
                selector_or_locator: impl Into<SelectorOrLocator>,
                options: LocatorOptions,
            ) -> Locator {
                let $frame = self;
                let frame: Frame = $get_frame;
                match selector_or_locator.into() {
                    SelectorOrLocator::Selector(s) => Locator::new(frame, s, Some(&options)),
                    SelectorOrLocator::Locator(other) => {
                        let mut l =
                            Locator::new(frame.clone(), other.selector.clone(), Some(&options));
                        if other.frame != frame {
                            l.fail("Locators must belong to the same frame.");
                        }
                        l.inherit_error(&other);
                        l
                    }
                }
            }

            /// Elements by test id.
            pub fn get_by_test_id(&self, test_id: impl Into<TextMatch>) -> Locator {
                self.locator(get_by_test_id_selector(
                    &test_id_attribute_name(),
                    &test_id.into(),
                ))
            }

            /// Elements by `alt` text.
            pub fn get_by_alt_text(&self, text: impl Into<TextMatch>) -> Locator {
                self.locator(get_by_alt_text_selector(&text.into(), false))
            }

            /// Elements by `alt` text, with options.
            pub fn get_by_alt_text_with(
                &self,
                text: impl Into<TextMatch>,
                options: GetByOptions,
            ) -> Locator {
                self.locator(get_by_alt_text_selector(&text.into(), options.exact))
            }

            /// Inputs by label text.
            pub fn get_by_label(&self, text: impl Into<TextMatch>) -> Locator {
                self.locator(get_by_label_selector(&text.into(), false))
            }

            /// Inputs by label text, with options.
            pub fn get_by_label_with(
                &self,
                text: impl Into<TextMatch>,
                options: GetByOptions,
            ) -> Locator {
                self.locator(get_by_label_selector(&text.into(), options.exact))
            }

            /// Inputs by placeholder.
            pub fn get_by_placeholder(&self, text: impl Into<TextMatch>) -> Locator {
                self.locator(get_by_placeholder_selector(&text.into(), false))
            }

            /// Inputs by placeholder, with options.
            pub fn get_by_placeholder_with(
                &self,
                text: impl Into<TextMatch>,
                options: GetByOptions,
            ) -> Locator {
                self.locator(get_by_placeholder_selector(&text.into(), options.exact))
            }

            /// Elements by text content.
            pub fn get_by_text(&self, text: impl Into<TextMatch>) -> Locator {
                self.locator(get_by_text_selector(&text.into(), false))
            }

            /// Elements by text content, with options.
            pub fn get_by_text_with(
                &self,
                text: impl Into<TextMatch>,
                options: GetByOptions,
            ) -> Locator {
                self.locator(get_by_text_selector(&text.into(), options.exact))
            }

            /// Elements by `title`.
            pub fn get_by_title(&self, text: impl Into<TextMatch>) -> Locator {
                self.locator(get_by_title_selector(&text.into(), false))
            }

            /// Elements by `title`, with options.
            pub fn get_by_title_with(
                &self,
                text: impl Into<TextMatch>,
                options: GetByOptions,
            ) -> Locator {
                self.locator(get_by_title_selector(&text.into(), options.exact))
            }

            /// Elements by ARIA role.
            pub fn get_by_role(&self, role: &str) -> Locator {
                self.get_by_role_with(role, GetByRoleOptions::default())
            }

            /// Elements by ARIA role, with options.
            pub fn get_by_role_with(&self, role: &str, options: GetByRoleOptions) -> Locator {
                self.locator(get_by_role_selector(role, &options.inner))
            }

            /// A frame locator for the `<iframe>` matching `selector`.
            pub fn frame_locator(&self, selector: &str) -> FrameLocator {
                let $frame = self;
                let frame: Frame = $get_frame;
                FrameLocator {
                    frame,
                    selector: selector.to_string(),
                    error: None,
                }
            }

            /// A frame locator matching in any frame of the subtree
            /// (`frameLocator()` without a selector).
            pub fn frame_locator_any(&self) -> FrameLocator {
                let $frame = self;
                let frame: Frame = $get_frame;
                FrameLocator {
                    frame,
                    selector: ANY_FRAME_SELECTOR.to_string(),
                    error: None,
                }
            }
        }
    };
}

locator_api!(Frame, f => f.clone());
locator_api!(Page, p => p.main_frame());

#[allow(dead_code)]
fn _unused() -> (WaitForSelectorState, Value) {
    (WaitForSelectorState::Visible, json!(null))
}

#[allow(dead_code)]
async fn _unused_value(ctx: &crate::execution_context::ExecutionContext) -> Result<()> {
    let _ = value_of(call_injected(ctx, "() => 1", vec![], true).await?);
    Ok(())
}
