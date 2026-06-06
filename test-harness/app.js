(function () {
  const imagePreview = 'data:image/svg+xml;utf8,' + encodeURIComponent(`
    <svg xmlns="http://www.w3.org/2000/svg" width="320" height="240" viewBox="0 0 320 240">
      <defs>
        <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stop-color="#2f7df6"/>
          <stop offset="1" stop-color="#22c55e"/>
        </linearGradient>
      </defs>
      <rect width="320" height="240" fill="url(#g)"/>
      <circle cx="232" cy="72" r="34" fill="#fff7"/>
      <path d="M32 198l76-82 54 58 42-44 84 68z" fill="#111827cc"/>
    </svg>
  `);
  const DOUBLE_CLICK_DELAY_MS = 220;

  class MockBackend {
    constructor() {
      this.items = [
        textItem('latest-path', '/home/sebnotseb/system76-ec-darp10-fan-curve-pr.md'),
        textItem('patch-path', '/home/sebnotseb/system76-ec-darp10-fan-curve.patch'),
        imageItem('image-hero', 'image/png', imagePreview),
        textItem('cmd-build', 'cargo build --release && gnome-extensions enable pastazzo@turinglabs.org'),
        textItem('note-bug', 'event.get_click_count is not available on GNOME 50'),
        textItem('url-doc', 'https://pasteapp.io/'),
        textItem('snippet-rust', 'fn touch_item(id: &str) -> Result<String, String> { /* ... */ }'),
        textItem('profile', 'Power Profile: Battery, CPU: 8% - 50%, No Turbo'),
        textItem('acpi', '/sys/class/hwmon/hwmon2/temp1_input'),
        textItem('journal', "journalctl --user -b | rg -i 'pastazzo|js error'"),
        textItem('ssh', 'ssh seb@turinglabs.org'),
        textItem('config', 'font-size = 11'),
      ];
      this.clipboard = null;
      this.beepCount = 0;
      this.copyCount = 0;
      this.closeCount = 0;
      this.clearCount = 0;
      this.pasteCount = 0;
      this.settingsOpenCount = 0;
    }

    search(query) {
      const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
      if (!terms.length)
        return this.items.slice();

      return this.items.filter(item => {
        const haystack = `${item.kind} ${item.mime ?? ''} ${item.text ?? ''}`.toLowerCase();
        return terms.every(term => haystack.includes(term));
      });
    }

    copyAndTouch(id) {
      const index = this.items.findIndex(item => item.id === id);
      if (index === -1)
        throw new Error(`Unknown item ${id}`);

      const [item] = this.items.splice(index, 1);
      item.timestamp = Date.now();
      this.items.unshift(item);
      this.clipboard = item.kind === 'image'
        ? {kind: 'image', mime: item.mime, previewUrl: item.previewUrl}
        : {kind: 'text', text: item.text};
      this.copyCount += 1;
      this.beepCount += 1;
      return item;
    }

    markClosed() {
      this.closeCount += 1;
    }

    clearHistory() {
      this.items = [];
      this.clearCount += 1;
      this.beepCount += 1;
    }

    openSettings() {
      this.settingsOpenCount += 1;
    }

    pasteToTarget(target) {
      if (!this.clipboard)
        return;

      this.pasteCount += 1;
      if (this.clipboard.kind === 'image') {
        target.value = '[image/png pasted]';
        return;
      }

      target.value = this.clipboard.text;
    }
  }

  function textItem(id, text) {
    return {
      id,
      kind: 'text',
      mime: 'text/plain',
      text,
      timestamp: Date.now(),
    };
  }

  function imageItem(id, mime, previewUrl) {
    return {
      id,
      kind: 'image',
      mime,
      previewUrl,
      text: '',
      timestamp: Date.now(),
    };
  }

  const backend = new MockBackend();
  const overlay = document.querySelector('[data-testid="overlay"]');
  const search = document.querySelector('[data-testid="search"]');
  const shelfScroll = document.querySelector('[data-testid="shelf-scroll"]');
  const shelf = document.querySelector('[data-testid="shelf"]');
  const settingsButton = document.querySelector('[data-testid="open-settings"]');
  const clearButton = document.querySelector('[data-testid="clear-history"]');
  const openButton = document.querySelector('[data-testid="open"]');
  const target = document.querySelector('[data-testid="target"]');

  let visibleItems = [];
  let clickTimeoutId = 0;
  let clickItemId = null;

  function openPastebar() {
    overlay.classList.add('is-open');
    overlay.setAttribute('aria-hidden', 'false');
    search.value = '';
    render();
    search.focus();
  }

  function closePastebar() {
    overlay.classList.remove('is-open');
    overlay.setAttribute('aria-hidden', 'true');
    backend.markClosed();
    target.focus();
  }

  function render() {
    visibleItems = backend.search(search.value);
    if (!visibleItems.length) {
      const empty = document.createElement('div');
      empty.className = 'pastebar-empty';
      empty.dataset.testid = 'empty';
      empty.textContent = 'No clipboard items';
      shelf.replaceChildren(empty);
      return;
    }

    shelf.replaceChildren(...visibleItems.map(renderCard));
  }

  function clearHistory() {
    if (clickTimeoutId) {
      clearTimeout(clickTimeoutId);
      clickTimeoutId = 0;
      clickItemId = null;
    }

    backend.clearHistory();
    search.value = '';
    render();
  }

  function openSettings() {
    if (clickTimeoutId) {
      clearTimeout(clickTimeoutId);
      clickTimeoutId = 0;
      clickItemId = null;
    }

    backend.openSettings();
    closePastebar();
  }

  function activate(item, paste) {
    backend.copyAndTouch(item.id);
    closePastebar();

    if (paste)
      backend.pasteToTarget(target);
  }

  function queueActivate(item) {
    if (clickTimeoutId && clickItemId === item.id) {
      clearTimeout(clickTimeoutId);
      clickTimeoutId = 0;
      clickItemId = null;
      activate(item, true);
      return;
    }

    if (clickTimeoutId)
      clearTimeout(clickTimeoutId);

    clickItemId = item.id;
    clickTimeoutId = window.setTimeout(() => {
      clickTimeoutId = 0;
      clickItemId = null;
      activate(item, false);
    }, DOUBLE_CLICK_DELAY_MS);
  }

  function renderCard(item, index) {
    const card = document.createElement('button');
    card.className = `pastebar-card${index === 0 ? ' is-selected' : ''}`;
    card.dataset.testid = 'card';
    card.dataset.cardId = item.id;
    card.type = 'button';

    const content = document.createElement('div');
    content.className = 'pastebar-card-content';

    const title = document.createElement('div');
    title.className = 'pastebar-card-title';
    title.textContent = item.kind === 'image' ? 'Image' : 'Text';

    const body = document.createElement('div');
    if (item.kind === 'image') {
      body.className = 'pastebar-image-frame';
      body.dataset.testid = 'image-preview';
      body.style.backgroundImage = `url("${item.previewUrl}")`;
    } else {
      body.className = 'pastebar-card-body';
      body.textContent = item.text;
    }

    const meta = document.createElement('div');
    meta.className = 'pastebar-card-meta';
    meta.textContent = item.kind === 'image' ? item.mime : `${item.text.length} characters`;

    content.append(title, body, meta);
    card.append(content);
    card.addEventListener('click', () => queueActivate(item));

    return card;
  }

  openButton.addEventListener('click', openPastebar);
  settingsButton.addEventListener('click', openSettings);
  clearButton.addEventListener('click', clearHistory);
  search.addEventListener('input', render);
  shelfScroll.addEventListener('wheel', event => {
    event.preventDefault();
    const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
    shelfScroll.scrollBy({
      left: delta * 28,
      behavior: 'smooth',
    });
  }, {passive: false});
  overlay.addEventListener('click', event => {
    if (event.target === overlay)
      closePastebar();
  });
  window.addEventListener('keydown', event => {
    if (event.key === 'Escape' && overlay.classList.contains('is-open'))
      closePastebar();
  });

  window.pastebarHarness = {
    open: openPastebar,
    close: closePastebar,
    state: () => ({
      beepCount: backend.beepCount,
      clearCount: backend.clearCount,
      clipboard: backend.clipboard,
      closeCount: backend.closeCount,
      copyCount: backend.copyCount,
      firstItemId: backend.items[0]?.id,
      itemIds: backend.items.map(item => item.id),
      isOpen: overlay.classList.contains('is-open'),
      pasteCount: backend.pasteCount,
      settingsOpenCount: backend.settingsOpenCount,
      targetText: target.value,
    }),
  };
})();
