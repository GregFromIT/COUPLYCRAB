//! Shopping list endpoints (shared by both users). See docs/API.md.

use crate::{
    api::{check_id, check_opt, check_text, created, some, Q, TITLE},
    events,
    identity::Me,
    ApiError, ApiResult, AppState, Body,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use shopping_core::{Item, ItemEdit, List, NewItem, Write};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/lists", get(list_lists))
        .route("/lists/{id}", put(put_list).patch(patch_list))
        .route("/lists/{id}/items", get(list_items))
        .route("/lists/{id}/items/{item_id}", put(put_item).patch(patch_item).delete(delete_item))
        .route("/lists/{id}/clear-checked", post(clear_checked))
        .route("/lists/{id}/suggestions", get(suggestions))
        .route("/events", get(events::stream))
}

const QUANTITY: usize = 50;
const NOTE: usize = 500;

// ---- lists ----

#[derive(Serialize)]
struct ListOut {
    id: String,
    name: String,
    created_at: i64,
    /// Items on the list that are not ticked.
    unchecked: usize,
    /// Items on the list (ticked or not).
    total: usize,
}

fn list_out(s: &dyn shopping_core::ShoppingStore, l: List) -> shopping_core::Result<ListOut> {
    let items = s.items(&l.id)?;
    Ok(ListOut { unchecked: items.iter().filter(|i| !i.checked()).count(), total: items.len(), id: l.id, name: l.name, created_at: l.created_at })
}

async fn list_lists(State(st): State<AppState>, _me: Me) -> ApiResult<Json<Vec<ListOut>>> {
    Ok(Json(st.read_shopping(|s| s.lists()?.into_iter().map(|l| list_out(s, l)).collect())?))
}

#[derive(Deserialize)]
struct ListIn {
    name: String,
}

async fn put_list(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Body(b): Body<ListIn>) -> ApiResult<Response> {
    check_id(&id)?;
    check_text("name", &b.name, TITLE, true)?;
    let w = st.write_shopping(|s| s.put_list(&id, &b.name))?;
    Ok(created(w == Write::Created, st.read_shopping(|s| list_out(s, s.get_list(&id)?))?))
}

#[derive(Deserialize)]
struct ListPatch {
    name: Option<String>,
    /// Only `true`: a deleted list cannot be brought back.
    deleted: Option<bool>,
}

async fn patch_list(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Body(b): Body<ListPatch>) -> ApiResult<Json<ListOut>> {
    if let Some(n) = &b.name {
        check_text("name", n, TITLE, true)?;
    }
    if b.deleted == Some(false) {
        return Err(ApiError::bad("deleted can only be set to true"));
    }
    st.write_shopping(|s| {
        s.get_list(&id)?;
        if let Some(n) = &b.name {
            s.rename_list(&id, n)?;
        }
        if b.deleted == Some(true) {
            s.delete_list(&id)?;
        }
        Ok(())
    })?;
    Ok(Json(st.read_shopping(|s| list_out(s, s.get_list(&id)?))?))
}

// ---- items ----

#[derive(Serialize)]
struct ItemOut {
    id: String,
    list_id: String,
    name: String,
    quantity: Option<String>,
    note: Option<String>,
    /// `produce meat dairy bakery pantry frozen household other`, or null.
    category: Option<String>,
    checked: bool,
    checked_by: Option<String>,
    /// Unix ms.
    checked_at: Option<i64>,
    added_by: String,
    added_at: i64,
}

impl From<Item> for ItemOut {
    fn from(i: Item) -> Self {
        ItemOut {
            checked: i.checked(),
            checked_by: i.checked_by().map(Into::into),
            checked_at: i.checked_at(),
            id: i.id,
            list_id: i.list_id,
            name: i.name,
            quantity: i.quantity,
            note: i.note,
            category: i.category,
            added_by: i.added_by,
            added_at: i.added_at,
        }
    }
}

async fn list_items(State(st): State<AppState>, _me: Me, Path(id): Path<String>) -> ApiResult<Json<Vec<ItemOut>>> {
    let items = st.read_shopping(|s| {
        s.get_list(&id)?;
        s.items(&id)
    })?;
    Ok(Json(items.into_iter().map(Into::into).collect()))
}

#[derive(Deserialize)]
struct ItemIn {
    name: String,
    quantity: Option<String>,
    note: Option<String>,
    category: Option<String>,
}

fn check_item_text(name: Option<&str>, quantity: Option<&String>, note: Option<&String>) -> ApiResult<()> {
    if let Some(n) = name {
        check_text("name", n, TITLE, true)?;
    }
    for (f, v, max) in [("quantity", quantity, QUANTITY), ("note", note, NOTE)] {
        check_opt(f, &v.cloned(), max)?;
    }
    Ok(())
}

async fn put_item(
    State(st): State<AppState>,
    Me(me): Me,
    Path((list_id, item_id)): Path<(String, String)>,
    Body(b): Body<ItemIn>,
) -> ApiResult<Response> {
    check_id(&item_id)?;
    check_item_text(Some(&b.name), b.quantity.as_ref(), b.note.as_ref())?;
    let new = NewItem { id: item_id.clone(), list_id, name: b.name, quantity: b.quantity, note: b.note, category: b.category };
    let w = st.write_shopping(|s| s.put_item(&me, new))?;
    Ok(created(w == Write::Created, ItemOut::from(st.read_shopping(|s| s.get_item(&item_id))?)))
}

#[derive(Deserialize)]
struct ItemPatch {
    name: Option<String>,
    #[serde(default, deserialize_with = "some")]
    quantity: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    note: Option<Option<String>>,
    #[serde(default, deserialize_with = "some")]
    category: Option<Option<String>>,
    /// Tick or untick (by the current user).
    checked: Option<bool>,
}

/// The item must be on this list.
fn item_of(s: &dyn shopping_core::ShoppingStore, list_id: &str, item_id: &str) -> shopping_core::Result<Item> {
    let it = s.get_item(item_id)?;
    if it.list_id == list_id { Ok(it) } else { Err(shopping_core::Error::NotFound(item_id.into())) }
}

async fn patch_item(
    State(st): State<AppState>,
    Me(me): Me,
    Path((list_id, item_id)): Path<(String, String)>,
    Body(b): Body<ItemPatch>,
) -> ApiResult<Json<ItemOut>> {
    check_item_text(b.name.as_deref(), b.quantity.as_ref().and_then(Option::as_ref), b.note.as_ref().and_then(Option::as_ref))?;
    let edit = ItemEdit { name: b.name, quantity: b.quantity, note: b.note, category: b.category };
    st.write_shopping(|s| {
        item_of(s, &list_id, &item_id)?;
        if edit != ItemEdit::default() {
            s.edit_item(&item_id, edit)?;
        }
        if let Some(c) = b.checked {
            s.set_checked(&me, &item_id, c)?;
        }
        Ok(())
    })?;
    Ok(Json(st.read_shopping(|s| s.get_item(&item_id))?.into()))
}

async fn delete_item(State(st): State<AppState>, _me: Me, Path((list_id, item_id)): Path<(String, String)>) -> ApiResult<StatusCode> {
    st.write_shopping(|s| {
        item_of(s, &list_id, &item_id)?;
        s.delete_item(&item_id)
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_checked(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Body(_): Body<serde_json::Value>) -> ApiResult<Json<serde_json::Value>> {
    let n = st.write_shopping(|s| s.clear_checked(&id))?;
    Ok(Json(serde_json::json!({ "cleared": n })))
}

async fn suggestions(State(st): State<AppState>, _me: Me, Path(id): Path<String>, Query(q): Q) -> ApiResult<Json<Vec<String>>> {
    let limit = match q.get("limit") {
        Some(l) => l.parse::<usize>().ok().filter(|l| (1..=20).contains(l)).ok_or_else(|| ApiError::bad("limit must be 1-20"))?,
        None => 8,
    };
    let text = q.get("q").map(String::as_str).unwrap_or("");
    Ok(Json(st.read_shopping(|s| {
        s.get_list(&id)?;
        s.suggestions(&id, text, limit)
    })?))
}
