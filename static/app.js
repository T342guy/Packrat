/* Garage Inventory — single-page frontend. No framework, no build step:
   the whole thing is served from the Rust binary. */

'use strict';

// ------------------------------------------------------------------ helpers

const $ = (sel, root = document) => root.querySelector(sel);
const view = () => $('#view');

const esc = (s) =>
  String(s ?? '').replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

function toast(message, isError = false) {
  const el = $('#toast');
  el.textContent = message;
  el.classList.toggle('error', isError);
  el.classList.add('show');
  clearTimeout(toast._timer);
  toast._timer = setTimeout(() => el.classList.remove('show'), 2600);
}

async function api(path, options = {}) {
  const res = await fetch(path, {
    headers: options.body && !(options.body instanceof FormData)
      ? { 'content-type': 'application/json' }
      : undefined,
    ...options,
  });
  const text = await res.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch { /* non-JSON response */ }
  if (!res.ok) throw new Error((data && data.error) || `request failed (${res.status})`);
  return data;
}

const state = {
  containers: [],
  tags: [],
  stats: {},
  kinds: [],
  publicUrl: '',
};

async function refreshState() {
  const data = await api('/api/bootstrap');
  state.containers = data.containers;
  state.tags = data.tags;
  state.stats = data.stats;
  state.kinds = data.kinds;
  state.publicUrl = data.public_url;
}

const containerById = (id) => state.containers.find((c) => c.id === Number(id));

const KIND_LABEL = {
  area: 'Area', shelf: 'Shelf', cabinet: 'Cabinet', drawer: 'Drawer',
  bin: 'Bin', box: 'Box', bag: 'Bag', other: 'Container',
};

// --------------------------------------------------------------- components

function itemRow(item, options = {}) {
  const photo = item.photo_id
    ? `<img class="thumb" src="/photos/${item.photo_id}" alt="" loading="lazy">`
    : '';
  // Inside a box view the location is the same for every row, so it's noise.
  const where = options.hideLocation
    ? ''
    : item.container_id
      ? `<div class="where"><a href="#/box/${encodeURIComponent(item.container_code)}"
           title="${esc(item.container_path)}">${esc(item.container_path)}</a></div>`
      : `<div class="where">Not in a box yet</div>`;
  const tags = item.tags.map((t) => `<a class="chip" href="#/items?tag=${encodeURIComponent(t)}">${esc(t)}</a>`).join('');
  return `
    <div class="row" data-item="${item.id}">
      ${photo}
      <div class="body">
        <div class="name">${esc(item.name)}</div>
        ${where}
        ${item.description ? `<div class="where" title="${esc(item.description)}">${esc(item.description)}</div>` : ''}
        ${tags ? `<div class="tagline">${tags}</div>` : ''}
      </div>
      <div class="side">
        <button class="tiny ghost" data-act="qty" data-id="${item.id}" data-delta="-1"
                aria-label="One fewer">&minus;</button>
        <span class="qty" data-qty="${item.id}">${item.quantity}</span>
        <button class="tiny ghost" data-act="qty" data-id="${item.id}" data-delta="1"
                aria-label="One more">+</button>
        <button class="tiny" data-act="edit-item" data-id="${item.id}">Edit</button>
      </div>
    </div>`;
}

function itemList(items, emptyMessage, options = {}) {
  if (!items.length) return `<div class="card"><div class="empty">${emptyMessage}</div></div>`;
  return `<div class="card list">${items.map((i) => itemRow(i, options)).join('')}</div>`;
}

function boxCard(container) {
  const parentPath = container.path.includes(' / ')
    ? container.path.slice(0, container.path.lastIndexOf(' / '))
    : '';
  const counts = [];
  if (container.item_count) counts.push(plural(container.item_count, 'item', 'items'));
  if (container.child_count) counts.push(plural(container.child_count, 'container', 'containers'));
  return `
    <a class="card box-card" href="#/box/${encodeURIComponent(container.code)}">
      <span class="chip code">${esc(container.code)}</span>
      <span class="title">${esc(container.name)}</span>
      ${parentPath ? `<span class="path">in ${esc(parentPath)}</span>` : ''}
      <span class="count">${counts.length ? counts.join(' · ') : 'Empty'}</span>
    </a>`;
}

function containerOptions(selectedId, excludeId) {
  const excluded = new Set();
  if (excludeId) {
    // Prevent picking a container that lives inside the one being edited.
    const walk = (id) => {
      excluded.add(id);
      state.containers.filter((c) => c.parent_id === id).forEach((c) => walk(c.id));
    };
    walk(Number(excludeId));
  }
  return state.containers
    .filter((c) => !excluded.has(c.id))
    .map((c) => {
      const indent = '  '.repeat(c.depth);
      const selected = Number(selectedId) === c.id ? ' selected' : '';
      return `<option value="${c.id}"${selected}>${indent}${esc(c.name)} (${esc(c.code)})</option>`;
    })
    .join('');
}

// -------------------------------------------------------------------- views

async function viewHome() {
  const recent = await api('/api/items?sort=newest&limit=8');
  const s = state.stats;
  // The most useful containers to show first are the ones holding the most.
  const highlights = [...state.containers]
    .sort((a, b) => (b.item_count - a.item_count) || (b.child_count - a.child_count))
    .slice(0, 6);

  const firstRun = state.stats.containers === 0 && state.stats.items === 0;
  if (firstRun) {
    view().innerHTML = `
      <h1>Let's get the garage sorted</h1>
      <p class="sub">Two steps: describe where things live, then list what's in them.</p>
      <div class="card settings-block">
        <h3>1. Add a place</h3>
        <p>Start with somewhere big — "Garage", "Shed", "Basement" — then add the shelves,
           cabinets and boxes inside it. Every container gets a printable code.</p>
        <button class="primary" data-act="add-box">Add a place or box</button>
      </div>
      <div class="card settings-block">
        <h3>2. Put things in it</h3>
        <p>Add items to a box as you pack it. Later you can search for anything and the app
           tells you which box it's in.</p>
        <button data-act="add-item">Add an item</button>
      </div>
      <div class="card settings-block">
        <h3>3. Print the labels</h3>
        <p>Each box gets a QR code. Scan it with any phone camera on this network and the box's
           contents open instantly — no need to unstack and open it.</p>
        <a class="btn" href="#/labels">Print labels</a>
      </div>`;
    return;
  }

  view().innerHTML = `
    <div class="stats">
      <div class="card stat"><div class="n">${s.items}</div><div class="l">distinct items</div></div>
      <div class="card stat"><div class="n">${s.total_quantity}</div><div class="l">things in total</div></div>
      <div class="card stat"><div class="n">${s.containers}</div><div class="l">boxes &amp; places</div></div>
      <div class="card stat"><div class="n">${s.unfiled_items}</div><div class="l">not filed yet</div></div>
    </div>

    <div class="actions">
      <button class="primary" data-act="add-item">+ Add item</button>
      <button data-act="add-box">+ Add box or place</button>
      <a class="btn" href="#/labels">Print labels</a>
      ${s.unfiled_items ? `<a class="btn" href="#/items?unfiled=1">Sort out ${s.unfiled_items} unfiled</a>` : ''}
    </div>

    <h2>Where things are</h2>
    ${highlights.length
      ? `<div class="grid">${highlights.map(boxCard).join('')}</div>
         ${state.containers.length > highlights.length
            ? `<p class="sub" style="margin-top:10px"><a href="#/boxes">See all ${state.containers.length} containers →</a></p>`
            : ''}`
      : `<div class="card"><div class="empty">Nothing yet.
           <button class="tiny" data-act="add-box">Add your first place</button></div></div>`}

    <h2>Recently added</h2>
    ${itemList(recent, 'No items yet.')}`;
}

async function viewSearch(query) {
  const data = await api(`/api/search?q=${encodeURIComponent(query)}`);
  const found = data.items.length + data.containers.length;
  view().innerHTML = `
    <h1>${found ? `${plural(found, 'match', 'matches')}` : 'Nothing found'} for “${esc(query)}”</h1>
    <p class="sub">${found
      ? 'Boxes come first, then the items inside them.'
      : 'Try a shorter word, or part of a box code such as BX.'}</p>
    ${data.containers.length
      ? `<h2>Boxes &amp; places</h2><div class="grid">${data.containers.map(boxCard).join('')}</div>`
      : ''}
    ${data.items.length ? `<h2>Items</h2>${itemList(data.items, '')}` : ''}
    ${found ? '' : `<div class="card"><div class="empty">
        <strong>No match</strong>Nothing in the inventory matches that yet.
        <div style="margin-top:10px"><button class="tiny primary" data-act="add-item"
          data-name="${esc(query)}">Add “${esc(query)}” as an item</button></div>
      </div></div>`}`;
}

async function viewBox(key) {
  const path = /^\d+$/.test(key)
    ? `/api/containers/${key}`
    : `/api/by-code/${encodeURIComponent(key)}`;
  const detail = await api(path);
  const c = detail.container;
  const crumbs = detail.ancestors
    .map((a) => `<a href="#/box/${encodeURIComponent(a.code)}">${esc(a.name)}</a>`)
    .concat([esc(c.name)])
    .join(' / ');
  const nested = detail.nested_item_count > c.item_count
    ? ` · ${detail.nested_item_count} including everything nested inside`
    : '';

  view().innerHTML = `
    <div class="crumbs"><a href="#/boxes">All containers</a> / ${crumbs}</div>
    <div class="card" style="margin-bottom:16px">
      ${c.photo_id ? `<img class="box-photo" src="/photos/${c.photo_id}" alt="">` : ''}
      <div class="box-head">
        <div class="info">
          <div class="code-big">${esc(c.code)}</div>
          <h1>${esc(c.name)}</h1>
          <p class="sub" style="margin:0">
            ${esc(KIND_LABEL[c.kind] || 'Container')} ·
            ${plural(c.item_count, 'item', 'items')}${nested}
          </p>
          ${c.notes ? `<div class="notes">${esc(c.notes)}</div>` : ''}
          <div class="actions" style="margin-top:12px">
            <button class="primary" data-act="add-item" data-container="${c.id}">+ Add item here</button>
            <button data-act="edit-box" data-id="${c.id}">Edit</button>
            <a class="btn" href="/labels?codes=${encodeURIComponent(c.code)}" target="_blank"
               rel="noopener">Print label</a>
          </div>
        </div>
        <div class="qr">
          <img src="/api/containers/${c.id}/qr.svg?size=240" alt="QR code for ${esc(c.code)}">
          <small>Scan to open this box</small>
        </div>
      </div>
    </div>

    ${detail.children.length
      ? `<h2>Inside this ${esc((KIND_LABEL[c.kind] || 'container').toLowerCase())}</h2>
         <div class="grid">${detail.children.map(boxCard).join('')}</div>`
      : ''}

    <h2>Contents</h2>
    ${itemList(detail.items, `This ${esc((KIND_LABEL[c.kind] || 'container').toLowerCase())} is empty.
       <div style="margin-top:10px"><button class="tiny primary" data-act="add-item"
         data-container="${c.id}">Add the first item</button></div>`, { hideLocation: true })}`;
}

async function viewItems(params) {
  const tag = params.get('tag') || '';
  const sort = params.get('sort') || 'name';
  const unfiled = params.get('unfiled') === '1';
  const query = new URLSearchParams({ sort });
  if (tag) query.set('tag', tag);
  if (unfiled) query.set('unfiled', 'true');
  const items = await api(`/api/items?${query}`);

  view().innerHTML = `
    <h1>All items</h1>
    <p class="sub">${plural(items.length, 'item', 'items')}${tag ? ` tagged “${esc(tag)}”` : ''}${
      unfiled ? ', not yet in a box' : ''}</p>
    <div class="card settings-block" style="padding:12px 14px">
      <div class="field-row">
        <label class="field">Tag
          <select data-filter="tag">
            <option value="">Any tag</option>
            ${state.tags.map((t) =>
              `<option value="${esc(t.name)}"${t.name === tag ? ' selected' : ''}>${esc(t.name)} (${t.item_count})</option>`).join('')}
          </select>
        </label>
        <label class="field">Sort by
          <select data-filter="sort">
            <option value="name"${sort === 'name' ? ' selected' : ''}>Name</option>
            <option value="newest"${sort === 'newest' ? ' selected' : ''}>Newest first</option>
            <option value="updated"${sort === 'updated' ? ' selected' : ''}>Recently changed</option>
            <option value="quantity"${sort === 'quantity' ? ' selected' : ''}>Quantity</option>
          </select>
        </label>
      </div>
      <label class="checkline" style="margin-top:10px">
        <input type="checkbox" data-filter="unfiled" ${unfiled ? 'checked' : ''}>
        Only things that aren't in a box yet
      </label>
    </div>
    ${itemList(items, 'No items match those filters.')}`;
}

async function viewBoxes() {
  const rows = state.containers
    .map((c) => {
      const indent = c.depth * 18;
      const counts = [];
      if (c.item_count) counts.push(plural(c.item_count, 'item', 'items'));
      if (c.child_count) counts.push(`${c.child_count} inside`);
      return `
        <div class="node" style="padding-left:${14 + indent}px">
          <a href="#/box/${encodeURIComponent(c.code)}" style="flex:1 1 auto; min-width:0">
            <span class="chip code">${esc(c.code)}</span>
            <strong>${esc(c.name)}</strong>
          </a>
          <span class="kind">${esc(KIND_LABEL[c.kind] || '')}${
            counts.length ? ` · ${counts.join(' · ')}` : ''}</span>
        </div>`;
    })
    .join('');

  view().innerHTML = `
    <h1>Boxes &amp; places</h1>
    <p class="sub">Containers nest: a garage holds shelves, a shelf holds boxes.</p>
    <div class="actions">
      <button class="primary" data-act="add-box">+ Add box or place</button>
      <a class="btn" href="#/labels">Print labels</a>
    </div>
    ${state.containers.length
      ? `<div class="card tree">${rows}</div>`
      : `<div class="card"><div class="empty"><strong>No containers yet</strong>
           Add the garage itself first, then the shelves and boxes inside it.</div></div>`}`;
}

async function viewLabels() {
  view().innerHTML = `
    <h1>Print labels</h1>
    <p class="sub">Each label carries a QR code. Scanning it on a phone connected to this
       network opens that box's contents — no unstacking, no opening.</p>
    ${state.containers.length === 0
      ? `<div class="card"><div class="empty"><strong>Nothing to label yet</strong>
           Add a box first.</div></div>`
      : `<div class="card" style="margin-bottom:14px">
           <div class="checklist" id="label-list">
             ${state.containers.map((c) => `
               <label>
                 <input type="checkbox" name="code" value="${esc(c.code)}">
                 <span class="chip code">${esc(c.code)}</span>
                 <span>${esc(c.path)}</span>
               </label>`).join('')}
           </div>
         </div>
         <div class="actions">
           <button data-act="label-all">Select all</button>
           <button data-act="label-none">Clear selection</button>
           <div style="flex:1 1 auto"></div>
           <button data-act="print-labels" data-size="small">Print small labels</button>
           <button class="primary" data-act="print-labels" data-size="large">
             Print labels with contents
           </button>
         </div>
         <p class="sub" style="margin-top:12px">Large labels list what's inside, so the label
            stays useful even without a phone. QR codes point at
            <code class="inline">${esc(state.publicUrl)}</code> —
            change that in <a href="#/settings">Settings</a> if it's wrong.</p>`}`;
}

async function viewSettings() {
  const settings = await api('/api/settings');
  const s = state.stats;
  const mb = (s.database_bytes / 1024 / 1024).toFixed(2);
  view().innerHTML = `
    <h1>Settings</h1>

    <div class="card settings-block">
      <h3>Address used in QR codes</h3>
      <p>Printed labels encode this address, so it must be reachable from a phone on your
         network. Detected: <code class="inline">${esc(settings.detected_url)}</code></p>
      <form data-form="settings">
        <label class="field">Base URL
          <input name="public_url" type="url" placeholder="${esc(settings.detected_url)}"
                 value="${esc(settings.public_url)}">
          <span class="hint">Leave empty to use the detected address. Set this if the server has
            a fixed IP or a hostname on your network.</span>
        </label>
        <div class="modal-actions" style="justify-content:flex-start;margin-top:10px">
          <button class="primary" type="submit">Save</button>
        </div>
      </form>
      <p style="margin-top:10px">Labels currently point at
         <code class="inline">${esc(settings.effective_public_url)}</code></p>
    </div>

    <div class="card settings-block">
      <h3>Backups</h3>
      <p>Everything lives in one SQLite file, but a JSON export is portable and readable.</p>
      <div class="actions">
        <a class="btn" href="/api/export" download>Export JSON</a>
        <a class="btn" href="/api/export?photos=true" download>Export JSON with photos</a>
        <a class="btn" href="/api/export.csv" download>Export CSV</a>
        <button data-act="import">Restore from JSON…</button>
      </div>
      <p class="hint" style="margin-top:10px">Restoring replaces the entire current inventory.</p>
    </div>

    <div class="card settings-block">
      <h3>What's stored</h3>
      <p style="margin:0">
        ${s.items} items (${s.total_quantity} things in total) ·
        ${s.containers} containers · ${s.tags} tags · ${s.photos} photos ·
        ${s.unfiled_items} unfiled · ${s.empty_containers} empty containers ·
        database ${mb} MB
      </p>
    </div>`;
}

// ------------------------------------------------------------------- modals

function closeModal() {
  $('#modal-root').innerHTML = '';
  document.body.style.overflow = '';
}

function openModal(html) {
  $('#modal-root').innerHTML = `<div class="modal-backdrop" data-backdrop>
      <div class="modal" role="dialog" aria-modal="true">${html}</div>
    </div>`;
  document.body.style.overflow = 'hidden';
  const first = $('#modal-root input:not([type=hidden]), #modal-root select');
  if (first) first.focus();
}

/** Shrinks a photo in the browser so phone snapshots don't bloat the database. */
async function preparePhoto(file, maxSide = 1400, quality = 0.82) {
  try {
    const bitmap = await createImageBitmap(file);
    const scale = Math.min(1, maxSide / Math.max(bitmap.width, bitmap.height));
    if (scale === 1 && file.size < 900 * 1024) return file;
    const canvas = document.createElement('canvas');
    canvas.width = Math.round(bitmap.width * scale);
    canvas.height = Math.round(bitmap.height * scale);
    canvas.getContext('2d').drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    const blob = await new Promise((resolve) => canvas.toBlob(resolve, 'image/jpeg', quality));
    return blob || file;
  } catch {
    return file; // Older browser: send the original and let the server decide.
  }
}

async function uploadPhoto(file) {
  const prepared = await preparePhoto(file);
  const form = new FormData();
  form.append('file', prepared, 'photo.jpg');
  const result = await api('/api/photos', { method: 'POST', body: form });
  return result.id;
}

function photoField(photoId) {
  return `
    <label class="field">Photo
      <div class="photo-picker">
        <img data-photo-preview src="${photoId ? `/photos/${photoId}` : ''}"
             alt="" ${photoId ? '' : 'style="visibility:hidden"'}>
        <div>
          <input type="hidden" name="photo_id" value="${photoId ?? ''}">
          <input type="file" name="photo_file" accept="image/*" capture="environment"
                 style="font-size:13px">
          <div class="hint">Optional. A photo of the contents makes a box searchable by eye.</div>
          ${photoId ? `<button type="button" class="tiny" data-act="clear-photo">Remove photo</button>` : ''}
        </div>
      </div>
    </label>`;
}

function itemModal(item, defaults = {}) {
  const isNew = !item;
  const data = item || {
    name: defaults.name || '',
    description: '',
    quantity: 1,
    container_id: defaults.container_id ?? null,
    photo_id: null,
    tags: [],
  };
  openModal(`
    <h3>${isNew ? 'Add an item' : 'Edit item'}</h3>
    <form data-form="item" data-id="${item ? item.id : ''}">
      <label class="field">What is it?
        <input name="name" required autocomplete="off" value="${esc(data.name)}"
               placeholder="Cordless drill">
      </label>
      <div class="field-row">
        <label class="field">How many
          <input name="quantity" type="number" min="0" step="1" value="${data.quantity}">
        </label>
        <label class="field">Where is it?
          <select name="container_id">
            <option value="">Not in a box yet</option>
            ${containerOptions(data.container_id)}
          </select>
        </label>
      </div>
      <label class="field">Notes
        <textarea name="description" placeholder="Battery is on the charger by the door"
        >${esc(data.description)}</textarea>
      </label>
      <label class="field">Tags
        <input name="tags" list="tag-suggestions" autocomplete="off"
               value="${esc(data.tags.join(', '))}" placeholder="tools, camping">
        <span class="hint">Comma separated. Tags make whole categories easy to pull up.</span>
      </label>
      <datalist id="tag-suggestions">
        ${state.tags.map((t) => `<option value="${esc(t.name)}">`).join('')}
      </datalist>
      ${photoField(data.photo_id)}
      <div class="modal-actions">
        ${isNew ? '' : `<button type="button" class="danger" data-act="del-item"
                         data-id="${item.id}">Delete</button>`}
        <div class="spacer"></div>
        <button type="button" data-act="close">Cancel</button>
        <button type="submit" class="primary">${isNew ? 'Add item' : 'Save'}</button>
      </div>
    </form>`);
}

function containerModal(container, defaults = {}) {
  const isNew = !container;
  const data = container || {
    name: '', kind: 'box', parent_id: defaults.parent_id ?? null,
    notes: '', photo_id: null, code: '',
  };
  openModal(`
    <h3>${isNew ? 'Add a box or place' : 'Edit container'}</h3>
    <form data-form="container" data-id="${container ? container.id : ''}">
      <label class="field">Name
        <input name="name" required autocomplete="off" value="${esc(data.name)}"
               placeholder="Camping gear">
      </label>
      <div class="field-row">
        <label class="field">Kind
          <select name="kind">
            ${state.kinds.map((k) =>
              `<option value="${k}"${k === data.kind ? ' selected' : ''}>${KIND_LABEL[k] || k}</option>`).join('')}
          </select>
        </label>
        <label class="field">Inside what?
          <select name="parent_id">
            <option value="">Nothing — it stands on its own</option>
            ${containerOptions(data.parent_id, container ? container.id : null)}
          </select>
        </label>
      </div>
      <label class="field">Notes
        <textarea name="notes" placeholder="Top shelf, behind the bikes">${esc(data.notes)}</textarea>
      </label>
      ${isNew ? '' : `<label class="field">Label code
        <input name="code" value="${esc(data.code)}" autocomplete="off">
        <span class="hint">Printed on the label and encoded in its QR code. Changing it means
          reprinting the label.</span>
      </label>`}
      ${photoField(data.photo_id)}
      <div class="modal-actions">
        ${isNew ? '' : `<button type="button" class="danger" data-act="del-box"
                         data-id="${container.id}">Delete</button>`}
        <div class="spacer"></div>
        <button type="button" data-act="close">Cancel</button>
        <button type="submit" class="primary">${isNew ? 'Add it' : 'Save'}</button>
      </div>
    </form>`);
}

// ------------------------------------------------------------- form submits

async function resolvePhotoId(form) {
  const file = form.querySelector('input[name=photo_file]').files[0];
  if (file) return await uploadPhoto(file);
  const existing = form.querySelector('input[name=photo_id]').value;
  return existing ? Number(existing) : null;
}

async function submitItem(form) {
  const id = form.dataset.id;
  const fd = new FormData(form);
  const payload = {
    name: fd.get('name'),
    description: fd.get('description') || '',
    quantity: Number(fd.get('quantity') || 1),
    container_id: fd.get('container_id') ? Number(fd.get('container_id')) : null,
    tags: String(fd.get('tags') || '').split(',').map((t) => t.trim()).filter(Boolean),
    photo_id: await resolvePhotoId(form),
  };
  const saved = await api(id ? `/api/items/${id}` : '/api/items', {
    method: id ? 'PUT' : 'POST',
    body: JSON.stringify(payload),
  });
  closeModal();
  toast(id ? 'Item saved' : `Added ${saved.name}`);
  await reload();
}

async function submitContainer(form) {
  const id = form.dataset.id;
  const fd = new FormData(form);
  const payload = {
    name: fd.get('name'),
    kind: fd.get('kind'),
    parent_id: fd.get('parent_id') ? Number(fd.get('parent_id')) : null,
    notes: fd.get('notes') || '',
    code: fd.get('code') || null,
    photo_id: await resolvePhotoId(form),
  };
  const saved = await api(id ? `/api/containers/${id}` : '/api/containers', {
    method: id ? 'PUT' : 'POST',
    body: JSON.stringify(payload),
  });
  closeModal();
  await refreshState();
  if (id) {
    toast('Container saved');
    await render();
  } else {
    toast(`${saved.name} is ${saved.code}`);
    location.hash = `#/box/${encodeURIComponent(saved.code)}`;
  }
}

async function submitSettings(form) {
  const fd = new FormData(form);
  const result = await api('/api/settings', {
    method: 'PUT',
    body: JSON.stringify({ public_url: fd.get('public_url') || '' }),
  });
  toast(`QR codes now point at ${result.effective_public_url}`);
  await reload();
}

// ------------------------------------------------------------------ actions

const actions = {
  'add-item': (el) => itemModal(null, {
    container_id: el.dataset.container || currentContainerId(),
    name: el.dataset.name,
  }),

  'edit-item': async (el) => {
    const item = await api(`/api/items/${el.dataset.id}`);
    itemModal(item);
  },

  'del-item': async (el) => {
    if (!confirm('Delete this item? This cannot be undone.')) return;
    await api(`/api/items/${el.dataset.id}`, { method: 'DELETE' });
    closeModal();
    toast('Item deleted');
    await reload();
  },

  qty: async (el) => {
    const item = await api(`/api/items/${el.dataset.id}/quantity`, {
      method: 'POST',
      body: JSON.stringify({ delta: Number(el.dataset.delta) }),
    });
    const cell = document.querySelector(`[data-qty="${item.id}"]`);
    if (cell) cell.textContent = item.quantity;
    state.stats.total_quantity += Number(el.dataset.delta);
  },

  'add-box': (el) => containerModal(null, { parent_id: el.dataset.parent || currentContainerId() }),

  'edit-box': (el) => containerModal(containerById(el.dataset.id)),

  'del-box': async (el) => {
    const container = containerById(el.dataset.id);
    const message = container && (container.item_count || container.child_count)
      ? `Delete ${container.name}?\n\nIts ${plural(container.item_count, 'item', 'items')} will `
        + 'become unfiled and anything nested inside moves up a level. Nothing is lost.'
      : 'Delete this container?';
    if (!confirm(message)) return;
    await api(`/api/containers/${el.dataset.id}`, { method: 'DELETE' });
    closeModal();
    toast('Container deleted');
    await refreshState();
    location.hash = '#/boxes';
  },

  'clear-photo': (el) => {
    const form = el.closest('form');
    form.querySelector('input[name=photo_id]').value = '';
    form.querySelector('input[name=photo_file]').value = '';
    const preview = form.querySelector('[data-photo-preview]');
    preview.removeAttribute('src');
    preview.style.visibility = 'hidden';
    el.remove();
  },

  'label-all': () => {
    document.querySelectorAll('#label-list input').forEach((i) => { i.checked = true; });
  },

  'label-none': () => {
    document.querySelectorAll('#label-list input').forEach((i) => { i.checked = false; });
  },

  'print-labels': (el) => {
    const codes = [...document.querySelectorAll('#label-list input:checked')].map((i) => i.value);
    if (!codes.length) {
      toast('Tick the boxes you want labels for first', true);
      return;
    }
    const url = `/labels?size=${el.dataset.size}&codes=${encodeURIComponent(codes.join(','))}`;
    window.open(url, '_blank', 'noopener');
  },

  import: () => {
    const picker = document.createElement('input');
    picker.type = 'file';
    picker.accept = 'application/json,.json';
    picker.onchange = async () => {
      const file = picker.files[0];
      if (!file) return;
      if (!confirm('Restoring replaces everything currently in the inventory. Continue?')) return;
      try {
        const result = await api('/api/import?confirm=replace', {
          method: 'POST',
          body: await file.text(),
        });
        toast(`Restored ${result.items} items and ${result.containers} containers`);
        await reload();
      } catch (err) {
        toast(err.message, true);
      }
    };
    picker.click();
  },

  close: () => closeModal(),
};

/** The box currently on screen, so "Add item" defaults to the right place. */
function currentContainerId() {
  const match = location.hash.match(/^#\/box\/(.+)$/);
  if (!match) return null;
  const key = decodeURIComponent(match[1]);
  const found = /^\d+$/.test(key)
    ? containerById(key)
    : state.containers.find((c) => c.code.toUpperCase() === key.toUpperCase());
  return found ? found.id : null;
}

// ------------------------------------------------------------------- router

function parseRoute() {
  const raw = location.hash.replace(/^#/, '') || '/';
  const [pathPart, queryPart] = raw.split('?');
  const segments = pathPart.split('/').filter(Boolean);
  return {
    name: segments[0] || 'home',
    arg: segments[1] ? decodeURIComponent(segments[1]) : null,
    params: new URLSearchParams(queryPart || ''),
  };
}

async function render() {
  const route = parseRoute();
  document.querySelectorAll('#tabs a').forEach((a) => {
    const active = a.dataset.route === route.name
      || (route.name === 'box' && a.dataset.route === 'boxes')
      || (route.name === 'search' && a.dataset.route === 'home');
    a.classList.toggle('active', active);
  });

  const searchInput = $('#search');
  if (route.name === 'search') {
    if (document.activeElement !== searchInput) searchInput.value = route.arg || '';
  } else if (document.activeElement !== searchInput) {
    searchInput.value = '';
  }
  $('#search-clear').hidden = !searchInput.value;
  $('#fab').hidden = route.name === 'settings' || route.name === 'labels';

  try {
    switch (route.name) {
      case 'home': await viewHome(); break;
      case 'search': await viewSearch(route.arg || ''); break;
      case 'box': await viewBox(route.arg); break;
      case 'boxes': await viewBoxes(); break;
      case 'items': await viewItems(route.params); break;
      case 'labels': await viewLabels(); break;
      case 'settings': await viewSettings(); break;
      default: location.hash = '#/';
    }
  } catch (err) {
    view().innerHTML = `<div class="card"><div class="empty">
        <strong>${esc(err.message)}</strong>
        <a href="#/">Back to the overview</a>
      </div></div>`;
  }
  window.scrollTo({ top: 0 });
}

/** Refetch shared state, then repaint the current view. */
async function reload() {
  await refreshState();
  await render();
}

// ------------------------------------------------------------------- wiring

document.addEventListener('click', async (event) => {
  if (event.target.matches('[data-backdrop]')) return closeModal();
  const el = event.target.closest('[data-act]');
  if (!el) return;
  const action = actions[el.dataset.act];
  if (!action) return;
  event.preventDefault();
  el.disabled = true;
  try {
    await action(el);
  } catch (err) {
    toast(err.message, true);
  } finally {
    el.disabled = false;
  }
});

document.addEventListener('submit', async (event) => {
  const form = event.target;
  const kind = form.dataset.form;
  if (!kind) return;
  event.preventDefault();
  const submitButton = form.querySelector('button[type=submit]');
  if (submitButton) submitButton.disabled = true;
  try {
    if (kind === 'item') await submitItem(form);
    else if (kind === 'container') await submitContainer(form);
    else if (kind === 'settings') await submitSettings(form);
  } catch (err) {
    toast(err.message, true);
  } finally {
    if (submitButton) submitButton.disabled = false;
  }
});

// Live preview when picking a photo.
document.addEventListener('change', async (event) => {
  const target = event.target;
  if (target.name === 'photo_file' && target.files[0]) {
    const preview = target.closest('.photo-picker').querySelector('[data-photo-preview]');
    preview.src = URL.createObjectURL(target.files[0]);
    preview.style.visibility = 'visible';
    return;
  }
  if (target.dataset.filter) {
    const params = parseRoute().params;
    if (target.type === 'checkbox') {
      if (target.checked) params.set(target.dataset.filter, '1');
      else params.delete(target.dataset.filter);
    } else if (target.value) {
      params.set(target.dataset.filter, target.value);
    } else {
      params.delete(target.dataset.filter);
    }
    location.hash = `#/items?${params}`;
  }
});

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && $('#modal-root').innerHTML) closeModal();
  // "/" focuses search, the way it does in most tools.
  if (event.key === '/' && !/^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement.tagName)) {
    event.preventDefault();
    $('#search').focus();
  }
});

let searchTimer = null;
$('#search').addEventListener('input', (event) => {
  const query = event.target.value.trim();
  $('#search-clear').hidden = !query;
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => {
    // replace(), not assignment: typing shouldn't fill the back button with
    // every prefix of the query.
    const target = query ? `#/search/${encodeURIComponent(query)}` : '#/';
    if (location.hash !== target) location.replace(target);
  }, 180);
});

$('#search-clear').addEventListener('click', () => {
  $('#search').value = '';
  $('#search-clear').hidden = true;
  location.replace('#/');
  $('#search').focus();
});

$('#fab').addEventListener('click', () => itemModal(null, { container_id: currentContainerId() }));

window.addEventListener('hashchange', render);

(async function start() {
  try {
    await refreshState();
    await render();
  } catch (err) {
    view().innerHTML = `<div class="card"><div class="empty">
      <strong>Could not reach the server</strong>${esc(err.message)}</div></div>`;
  }
})();
