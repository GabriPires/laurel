// Settings → Tags (spec library.tags): the standard tags, fixed, and the custom
// tags with Rename and Delete, each with how many items have it outside Recently
// deleted, as the library's side panel counts them; and a field that creates a
// custom tag under the picker's rules (the index's, library_tag_create). Deleting
// asks first, counting the items in Recently deleted too. The names come from the
// library's tag catalog (tagCatalog, library.js), read again after each change, so
// that a renamed tag shows its new name everywhere.

// counts: library_tag_counts. editing: the custom tag being renamed, with the name
// typed (draft); confirming: the one whose delete waits for the confirmation;
// error: why the index refused a rename or a delete ({ id, text }); busy: a change
// on its way, while the buttons wait.
const tagsPage = { counts: {}, editing: null, draft: '', confirming: null, error: null, busy: false };

const tagItems = (id) => tagsPage.counts[id]?.items ?? 0;

function tagsPageEl(element, className, text) {
  const el = document.createElement(element);
  el.className = className;
  if (text != null) el.textContent = text;
  return el;
}

// A button of a custom tag's row; `focus` names it in its row (see renderTagsPage).
function tagsPageButton(text, className, focus, onclick) {
  const button = tagsPageEl('button', `btn ${className}`, text);
  button.dataset.focus = focus;
  button.disabled = tagsPage.busy;
  button.onclick = onclick;
  return button;
}

// A name cut with "…" when it does not fit, whole on hover.
function tagRowName(name) {
  const el = tagsPageEl('span', 'tag-row-name', name);
  el.title = name;
  return el;
}

const tagRowCount = (id) => tagsPageEl('span', 'tag-row-count', tp(tagItems(id), '{n} item', '{n} items'));

// The confirmation of a delete: every item that has the tag, those in Recently
// deleted too, which it says apart.
function deleteQuestion(tag) {
  const { items = 0, deleted = 0 } = tagsPage.counts[tag.id] ?? {};
  if (items + deleted === 0) return t('Delete “{name}”? No item has it.', { name: tag.name });
  const question = tp(items + deleted, 'Remove “{name}” from {n} item?', 'Remove “{name}” from {n} items?', { name: tag.name });
  if (!deleted) return question;
  const there = items
    ? tp(deleted, '{n} of them is in Recently deleted.', '{n} of them are in Recently deleted.')
    : tp(deleted, 'It is in Recently deleted.', 'They are all in Recently deleted.');
  return `${question} ${there}`;
}

function customTagRow(tag) {
  const li = tagsPageEl('li', 'tag-row');
  li.dataset.id = tag.id;
  if (tagsPage.editing === tag.id) {
    const input = tagsPageEl('input', 'tag-rename-input');
    input.dataset.focus = 'name';
    input.type = 'text';
    input.value = tagsPage.draft;
    input.spellcheck = false;
    input.autocomplete = 'off';
    input.setAttribute('aria-label', t('New name for “{name}”', { name: tag.name }));
    input.oninput = () => {
      tagsPage.draft = input.value;
      if (tagsPage.error?.id === tag.id) {
        tagsPage.error = null;
        li.querySelector('.tag-error')?.remove();
      }
    };
    input.onkeydown = (e) => {
      if (e.key === 'Enter') renameTag(tag);
      else if (e.key === 'Escape') stopRenaming(tag.id);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    li.append(input, tagsPageButton(t('Save'), 'btn-primary', 'save', () => renameTag(tag)), tagsPageButton(t('Cancel'), 'btn-secondary', 'cancel', () => stopRenaming(tag.id)));
  } else {
    li.append(tagRowName(tag.name), tagRowCount(tag.id));
    li.append(tagsPageButton(t('Rename'), 'btn-secondary', 'rename', () => {
      Object.assign(tagsPage, { editing: tag.id, draft: tag.name, confirming: null, error: null });
      renderTagsPage();
      focusTagRow(tag.id, 'name')?.select();
    }));
    li.append(tagsPageButton(t('Delete'), 'btn-danger-outline', 'delete', () => {
      Object.assign(tagsPage, { editing: null, confirming: tag.id, error: null });
      renderTagsPage();
      focusTagRow(tag.id, 'delete-yes');
    }));
    if (tagsPage.confirming === tag.id) {
      const confirm = tagsPageEl('div', 'confirm danger full');
      const row = tagsPageEl('div', 'row');
      row.append(tagsPageButton(t('Delete tag'), 'btn-danger', 'delete-yes', () => deleteTag(tag)), tagsPageButton(t('Cancel'), 'btn-secondary', 'delete-no', () => {
        tagsPage.confirming = null;
        renderTagsPage();
        focusTagRow(tag.id, 'delete');
      }));
      confirm.append(tagsPageEl('p', '', deleteQuestion(tag)), row);
      li.append(confirm);
    }
  }
  if (tagsPage.error?.id === tag.id) {
    const error = tagsPageEl('p', 'tag-error full', tagsPage.error.text);
    error.setAttribute('role', 'alert');
    li.append(error);
  }
  return li;
}

// Focuses the control named `focus` in the row of the custom tag `id`, and returns it.
function focusTagRow(id, focus) {
  const row = [...$('tags-custom').children].find((li) => li.dataset.id === id);
  const el = row?.querySelector(`[data-focus="${focus}"]`);
  el?.focus();
  return el;
}

function renderTagsPage() {
  // The rows are drawn again on every change of the library (the counts): the
  // control that had the focus gets it back, and the name being typed keeps its
  // text and where the cursor is.
  const focused = document.activeElement;
  const row = focused?.closest('#tags-custom li');
  const kept = row && focused.dataset.focus ? { id: row.dataset.id, focus: focused.dataset.focus, start: focused.selectionStart, end: focused.selectionEnd } : null;

  const groups = Object.fromEntries(tagCatalog.groups.map((group) => [group.id, group.name]));
  $('tags-standard').replaceChildren(...tagCatalog.tags.filter((tag) => tag.group !== 'custom').map((tag) => {
    const li = tagsPageEl('li', 'tag-row');
    li.append(tagRowName(tag.name), tagsPageEl('span', 'tag-row-group', groups[tag.group] ?? ''), tagRowCount(tag.id));
    return li;
  }));

  const custom = tagCatalog.tags.filter((tag) => tag.group === 'custom');
  // A tag renamed or deleted meanwhile (in the picker, say) leaves its editing.
  if (!custom.some((tag) => tag.id === tagsPage.editing)) tagsPage.editing = null;
  if (!custom.some((tag) => tag.id === tagsPage.confirming)) tagsPage.confirming = null;
  $('tags-custom').replaceChildren(...custom.map(customTagRow));
  if (!custom.length) {
    $('tags-custom').replaceChildren(tagsPageEl('li', 'tag-row muted', t('No tags of your own yet: create one below, or in the tag picker of a clip.')));
  }

  const again = kept && focusTagRow(kept.id, kept.focus);
  if (again && kept.focus === 'name') again.setSelectionRange(kept.start, kept.end);
  $('tags-create').disabled = tagsPage.busy || !trimName($('tags-new').value);
}

// The catalog and the counts, read again: after a change here, and when the
// library changes while the page is open.
async function loadTagsPage() {
  try {
    const [counts] = await Promise.all([invoke('library_tag_counts'), loadTagCatalog()]);
    tagsPage.counts = counts;
  } catch (e) {
    $('settings-status').textContent = t('Tags unavailable: {error}', { error: e });
  }
  renderTagsPage();
}

// Runs a change of the custom tags, one at a time; `refused` gets the index's reason.
async function changeCustomTags(change, refused) {
  if (tagsPage.busy) return false;
  tagsPage.busy = true;
  renderTagsPage();
  try {
    await change();
    return true;
  } catch (e) {
    refused(`${e}`);
    return false;
  } finally {
    tagsPage.busy = false;
    await loadTagsPage();
  }
}

function stopRenaming(id) {
  Object.assign(tagsPage, { editing: null, error: null });
  renderTagsPage();
  focusTagRow(id, 'rename');
}

async function renameTag(tag) {
  if (trimName(tagsPage.draft) === tag.name) {
    stopRenaming(tag.id);
    return;
  }
  let renamed;
  const done = await changeCustomTags(async () => {
    renamed = await invoke('library_tag_rename', { id: tag.id, name: tagsPage.draft });
  }, (text) => {
    tagsPage.error = { id: tag.id, text };
  });
  if (!done) {
    focusTagRow(tag.id, 'name');
    return;
  }
  $('settings-status').textContent = t('Tag renamed to “{name}”', { name: renamed.name });
  stopRenaming(tag.id);
}

async function deleteTag(tag) {
  const done = await changeCustomTags(() => invoke('library_tag_delete', { id: tag.id }), (text) => {
    tagsPage.error = { id: tag.id, text };
  });
  if (!done) return;
  tagsPage.confirming = null;
  leaveTagFilter(tag.id);
  $('settings-status').textContent = t('Tag “{name}” deleted', { name: tag.name });
  renderTagsPage();
  $('tags-new').focus();
}

async function createTagFromPage() {
  const input = $('tags-new');
  const name = input.value;
  if (!trimName(name)) return;
  let created;
  const done = await changeCustomTags(async () => {
    created = await invoke('library_tag_create', { name, paths: [] });
  }, (text) => {
    $('tags-new-error').textContent = text;
    $('tags-new-error').hidden = false;
  });
  if (!done) return;
  input.value = '';
  $('settings-status').textContent = t('Tag “{name}” created', { name: created.tag.name });
  renderTagsPage();
  input.focus();
}

$('tags-new').oninput = () => {
  $('tags-new-error').hidden = true;
  $('tags-create').disabled = tagsPage.busy || !trimName($('tags-new').value);
};
$('tags-new').onkeydown = (e) => {
  if (e.key !== 'Enter') return;
  e.preventDefault();
  createTagFromPage();
};
$('tags-create').onclick = createTagFromPage;

const openSettingsBeforeTags = window.openSettings;
window.openSettings = (...args) => {
  openSettingsBeforeTags?.(...args);
  loadTagsPage();
};
listen('library-changed', () => { if (!$('view-settings').hidden && !tagsPage.busy) loadTagsPage(); });
