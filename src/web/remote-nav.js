// Remote-control mode: arrows, Enter and Escape (or Backspace) drive the web
// client the way a TV remote would. Off by default; the remote button in the
// header switches it, and the choice is remembered. While it is off this file
// only adds that button and leaves every key alone.
//
// Navigation works on rows, not on screen distance: a horizontal scroller is
// one row, everything else is grouped by the line it sits on. Left/Right move
// within a row, Up/Down move to the neighbouring row and come back to the item
// last used in it. Escape goes back and lands on the item you came from.
(function () {
    var STORE_KEY = 'jellium.remoteMode';
    var ACCENT = '#00a4dc';

    var enabled = false;
    try { enabled = localStorage.getItem(STORE_KEY) === '1'; } catch (e) { /* storage blocked */ }

    var current = null;              // focused unit (card, list item, button, ...)
    var opener = null;               // unit last activated outside a menu or dialog
    var rowMemory = new WeakMap();   // row key element -> last unit used in it
    var pageMemory = {};             // location.hash -> descriptor of the unit to restore

    // ---- styles -------------------------------------------------------------

    var style = document.createElement('style');
    style.textContent =
        'html.jrn-on [data-jrn-focus]:not(.card):not(.listItem){outline:3px solid ' + ACCENT + ' !important;outline-offset:2px;border-radius:4px}' +
        // The ring is drawn inside the poster: scrollers clip anything outside it.
        'html.jrn-on .card[data-jrn-focus] .cardBox{transform:scale(1.04);transition:transform .12s ease-out}' +
        'html.jrn-on .card[data-jrn-focus] .cardScalable::after{content:"";position:absolute;inset:0;z-index:5;pointer-events:none;' +
            'border:4px solid ' + ACCENT + ';border-radius:inherit}' +
        // The hover overlay (play, played, favourite, more) would dim the focused poster.
        'html.jrn-on:not(.jrn-mouse) .card[data-jrn-focus] .cardOverlayContainer{opacity:0 !important}' +
        'html.jrn-on .listItem[data-jrn-focus]{outline:3px solid ' + ACCENT + ' !important;outline-offset:-3px;background:rgba(0,164,220,.15)}' +
        'html.jrn-on.jrn-mouse [data-jrn-focus]{outline:none !important}' +
        'html.jrn-on.jrn-mouse .card[data-jrn-focus] .cardBox{transform:none}' +
        'html.jrn-on.jrn-mouse .card[data-jrn-focus] .cardScalable::after{display:none}' +
        '#jrn-toggle{display:inline-flex;align-items:center;justify-content:center;width:40px;height:40px;margin:0 4px;' +
            'border:0;border-radius:50%;background:transparent;color:inherit;cursor:pointer;padding:0}' +
        '#jrn-toggle:hover{background:rgba(255,255,255,.08)}' +
        '#jrn-toggle.on{color:' + ACCENT + '}' +
        '#jrn-toggle .material-icons{font-size:24px}';

    // ---- what can be focused --------------------------------------------------

    var CANDIDATES = 'button, a[href], .card, .listItem, input, select, textarea, ' +
        '[role="button"], [role="tab"], [role="menuitem"], [role="option"], [tabindex="0"]';

    function isShown(el) {
        if (!el.isConnected) return false;
        var r = el.getBoundingClientRect();
        if (r.width < 2 || r.height < 2) return false;
        if (el.closest('.hide, [hidden], [aria-hidden="true"]')) return false;
        var cs = getComputedStyle(el);
        return cs.visibility !== 'hidden' && cs.display !== 'none' && parseFloat(cs.opacity) > 0.05;
    }

    function isModal(el) {
        return isShown(el) || (el.firstElementChild && isShown(el.firstElementChild));
    }

    // Topmost open menu or dialog, if any; navigation stays inside it.
    function openOverlay() {
        var list = [].slice.call(document.querySelectorAll(
            '.MuiModal-root, .MuiPopover-root, .dialogContainer .dialog.opened, .actionSheet.opened'));
        for (var i = list.length - 1; i >= 0; i--) {
            var el = list[i];
            if (el.classList.contains('MuiModal-hidden')) continue;
            if (isModal(el)) return el;
        }
        return null;
    }

    function shownPages() {
        return [].filter.call(document.querySelectorAll('.mainAnimatedPage:not(.hide), .page:not(.hide)'), isShown);
    }

    function activePage() {
        var pages = shownPages();
        return pages.length ? pages[pages.length - 1] : null;
    }

    // The React header on current servers, the older one elsewhere; both can
    // be in the document, only one is shown.
    function header() {
        var list = document.querySelectorAll('header.MuiAppBar-root, .skinHeader');
        for (var i = 0; i < list.length; i++) if (isShown(list[i])) return list[i];
        return null;
    }

    // A card is one stop. A list row (an episode) is one stop for its title
    // and play button, plus one per button at its right end (info, played,
    // favourite, more).
    function unitOf(el) {
        var card = el.closest('.card');
        if (card) return card;
        var li = el.closest('.listItem');
        if (li && (el === li || !el.closest('.listViewUserDataButtons, .listItemButton'))) return li;
        return el;
    }

    function skip(el) {
        if (el.disabled || el.getAttribute('aria-disabled') === 'true') return true;
        // The A-Z jump list beside a library grid: thirty single letters in a column.
        if (el.closest('.alphaPicker, .MuiToggleButtonGroup-vertical')) return true;
        if (el.closest('.MuiBackdrop-root')) return true;
        // "Recently Added in Movies >" above a row: the header reaches the same place.
        if (el.classList.contains('sectionTitleTextButton')) return true;
        // Detail page extras a remote has no use for: the poster, tag, genre,
        // studio, people and external-site links.
        if (el.closest('.detailImageContainer, .itemTags, .itemExternalLinks, .detailsGroupItem')) return true;
        return false;
    }

    function units(scope) {
        var seen = new Set();
        var out = [];
        scope.forEach(function (root) {
            if (!root) return;
            root.querySelectorAll(CANDIDATES).forEach(function (el) {
                var u = unitOf(el);
                if (seen.has(u) || skip(u) || !isShown(u)) return;
                seen.add(u);
                out.push(u);
            });
        });
        return out;
    }

    function currentScope() {
        var overlay = openOverlay();
        if (overlay) return [overlay];
        return [header(), activePage()];
    }

    // ---- rows ---------------------------------------------------------------

    function scrollerOf(u) {
        return u.closest('.emby-scroller.scrollX, .scrollX');
    }

    function rect(u) {
        return u.getBoundingClientRect();
    }

    function byLeft(a, b) {
        return rect(a).left - rect(b).left;
    }

    // Rows top to bottom; each row is {key, items[], mid} with items left to right.
    function buildRows(list) {
        var rows = [];
        var byScroller = new Map();
        var loose = [];
        list.forEach(function (u) {
            var s = (u.classList.contains('card') && scrollerOf(u)) || u.closest('.listItem');
            if (s) {
                if (!byScroller.has(s)) {
                    var row = { key: s, items: [] };
                    byScroller.set(s, row);
                    rows.push(row);
                }
                byScroller.get(s).items.push(u);
            } else {
                loose.push(u);
            }
        });
        rows.forEach(function (row) {
            var r = rect(row.key);
            row.mid = r.top + r.height / 2;
            row.items.sort(byLeft);
        });

        // Group the rest by line: items whose vertical centres are close,
        // measured against the smaller of the two heights, share a line.
        var mids = new Map();
        loose.forEach(function (u) {
            var r = rect(u);
            mids.set(u, { mid: r.top + r.height / 2, h: r.height });
        });
        loose.sort(function (a, b) { return mids.get(a).mid - mids.get(b).mid; });
        var lines = [];
        loose.forEach(function (u) {
            var m = mids.get(u);
            var line = lines[lines.length - 1];
            if (!line || Math.abs(m.mid - line.mid) > Math.max(6, Math.min(m.h, line.h) / 2)) {
                line = { items: [], mid: m.mid, h: m.h };
                lines.push(line);
            }
            line.items.push(u);
        });
        lines.forEach(function (line) {
            line.items.sort(byLeft);
            // Keyed by the first item's parent, so the memory survives a re-layout.
            line.key = line.items[0].parentElement || line.items[0];
        });

        var all = rows.concat(lines);
        var hdr = header();
        all.sort(function (a, b) {
            var ah = hdr && hdr.contains(a.items[0]) ? 0 : 1;
            var bh = hdr && hdr.contains(b.items[0]) ? 0 : 1;
            if (ah !== bh) return ah - bh;
            return a.mid - b.mid;
        });
        return all;
    }

    function locate(rows, u) {
        for (var i = 0; i < rows.length; i++) {
            var j = rows[i].items.indexOf(u);
            if (j >= 0) return { row: i, col: j };
        }
        return null;
    }

    // ---- focus --------------------------------------------------------------

    // What a card opens: its link to the item. Older cards carry it on
    // .cardContent, the React library grid on an anchor in the overlay.
    // Tried in this order (querySelector alone would take document order).
    var CARD_LINK = ['a.cardImageContainer[href]', 'a.cardContent[href]', '.cardContent[data-action]',
        '.cardContent', 'a[href]', 'button'];

    function firstOf(root, selectors) {
        for (var i = 0; i < selectors.length; i++) {
            var el = root.querySelector(selectors[i]);
            if (el) return el;
        }
        return root;
    }

    function cardLink(card) {
        return firstOf(card, CARD_LINK);
    }

    // An episode row plays (or resumes) the episode, like its play button.
    var LIST_PLAY = ['[data-action="resume"]', '[data-action="play"]', '[data-action="link"]', '.itemAction'];

    function focusTarget(u) {
        return u.classList.contains('card') ? cardLink(u) : u;
    }

    // The highlight is an attribute, not a class: React rewrites the class
    // list of the header buttons it renders.
    function setCurrent(u, rows) {
        if (current && current !== u) current.removeAttribute('data-jrn-focus');
        current = u;
        if (!u) return;
        u.setAttribute('data-jrn-focus', '');
        document.documentElement.classList.remove('jrn-mouse');
        try { focusTarget(u).focus({ preventScroll: true }); } catch (e) { /* not focusable */ }
        if (rows) {
            var at = locate(rows, u);
            if (at) rowMemory.set(rows[at.row].key, u);
        }
        reveal(u);
        // Menu and dialog items are gone by the time you come back to a page.
        if (!openOverlay()) pageMemory[location.hash] = describe(u);
    }

    // Nearest ancestor that scrolls along the axis ('x' or 'y'), if any.
    function scrollParent(el, axis) {
        for (var p = el.parentElement; p && p !== document.body && p !== document.documentElement; p = p.parentElement) {
            var o = getComputedStyle(p)[axis === 'x' ? 'overflowX' : 'overflowY'];
            if ((o === 'auto' || o === 'scroll') &&
                (axis === 'x' ? p.scrollWidth > p.clientWidth : p.scrollHeight > p.clientHeight)) return p;
        }
        return null;
    }

    // Scrolls only the row and the page (or the dialog) the item sits in.
    // scrollIntoView would also shift containers that are not meant to
    // scroll, such as the page itself sideways.
    function reveal(u) {
        var hdr = header();
        if (hdr && hdr.contains(u)) return;
        var s = scrollerOf(u);
        if (s) {
            var sr = rect(s);
            var ur = rect(u);
            if (ur.left < sr.left || ur.right > sr.right) {
                var dx = ur.left + ur.width / 2 - (sr.left + sr.width / 2);
                // emby-scroller moves its slider itself.
                if (typeof s.scrollToPosition === 'function' && typeof s.getScrollPosition === 'function') {
                    s.scrollToPosition(Math.max(0, s.getScrollPosition() + dx), false);
                } else {
                    var hx = scrollParent(u, 'x');
                    if (hx) hx.scrollLeft += dx;
                }
            }
        }
        var r = rect(u);
        var box = scrollParent(u, 'y');
        var top = box ? rect(box).top : (hdr ? rect(hdr).bottom : 0);
        var bottom = box ? rect(box).bottom : window.innerHeight;
        if (r.top < top || r.bottom > bottom) {
            var dy = r.top + r.height / 2 - (top + bottom) / 2;
            if (box) box.scrollTop += dy;
            else window.scrollBy(0, dy);
        }
    }

    // How to find an item again after its page was rebuilt: the Jellyfin
    // item it belongs to (a button in an episode row belongs to that
    // episode), plus its label to tell that item's buttons apart.
    function describe(u) {
        var idEl = u.hasAttribute('data-id') ? u : (u.querySelector('[data-id]') || u.closest('[data-id]'));
        return {
            id: idEl ? idEl.getAttribute('data-id') : null,
            text: (u.getAttribute('aria-label') || u.getAttribute('title') || u.textContent || '').trim().slice(0, 80)
        };
    }

    function findDescribed(list, d) {
        if (!d) return null;
        var sameId = [];
        var sameText = null;
        for (var i = 0; i < list.length; i++) {
            var other = describe(list[i]);
            if (d.id && other.id === d.id) {
                if (other.text === d.text) return list[i];
                sameId.push(list[i]);
            } else if (!d.id && !sameText && other.text === d.text && !list[i].classList.contains('card')) {
                sameText = list[i];
            }
        }
        return sameId[0] || sameText;
    }

    function isContent(u) {
        return u.classList.contains('card') || u.classList.contains('listItem');
    }

    // Where focus starts on a page: the item you left it from, else the play
    // button of a detail page, else the first card or list item.
    function initial(list) {
        var back = findDescribed(list, pageMemory[location.hash]);
        if (back) return back;
        var hdr = header();
        var body = list.filter(function (u) { return !(hdr && hdr.contains(u)); });
        var play = body.filter(function (u) {
            return !isContent(u) && !u.closest('.card, .listItem') &&
                u.matches('.btnPlay, .btnResume, [data-action="resume"], [data-action="play"]');
        });
        if (play.length) return play[0];
        var rows = buildRows(body);
        for (var i = 0; i < rows.length; i++) {
            var first = rows[i].items.filter(isContent)[0];
            if (first) return first;
        }
        return body[0] || list[0] || null;
    }

    // When a menu or dialog closes, its items go away; carry on from the
    // button that opened it.
    function ensureCurrent(list) {
        if (current && list.indexOf(current) >= 0) return current;
        if (current) current.removeAttribute('data-jrn-focus');
        current = null;
        if (opener && list.indexOf(opener) >= 0) {
            current = opener;
            current.setAttribute('data-jrn-focus', '');
            return current;
        }
        return null;
    }

    // ---- moving -------------------------------------------------------------

    function centerX(u) {
        var r = rect(u);
        return r.left + r.width / 2;
    }

    function move(dir) {
        var list = units(currentScope());
        if (!list.length) return;
        var rows = buildRows(list);
        var at = ensureCurrent(list) && locate(rows, current);
        if (!at) {
            setCurrent(initial(list), rows);
            return;
        }
        var row = rows[at.row];
        if (dir === 'left' || dir === 'right') {
            var j = at.col + (dir === 'right' ? 1 : -1);
            if (j >= 0 && j < row.items.length) setCurrent(row.items[j], rows);
            return;
        }
        var k = at.row + (dir === 'down' ? 1 : -1);
        if (k < 0 || k >= rows.length) return;
        var target = rows[k];
        if (target.key.classList.contains('listItem') && !current.closest('.listItem')) {
            setCurrent(target.items[0], rows);
            return;
        }
        var mem = rowMemory.get(target.key);
        if (mem && target.items.indexOf(mem) >= 0) {
            setCurrent(mem, rows);
            return;
        }
        var x = centerX(current);
        var best = target.items[0];
        var bestD = Infinity;
        target.items.forEach(function (u) {
            var d = Math.abs(centerX(u) - x);
            if (d < bestD) { bestD = d; best = u; }
        });
        setCurrent(best, rows);
    }

    function activate() {
        var list = units(currentScope());
        if (!ensureCurrent(list)) {
            setCurrent(initial(list), buildRows(list));
            return;
        }
        var u = current;
        if (!openOverlay()) opener = u;
        if (u.classList.contains('card')) {
            cardLink(u).click();
        } else if (u.classList.contains('listItem')) {
            firstOf(u, LIST_PLAY).click();
        } else if (u.tagName === 'INPUT' && !/^(checkbox|radio|button|submit)$/i.test(u.type) || u.tagName === 'TEXTAREA') {
            u.focus();
        } else {
            u.click();
        }
    }

    function isHome() {
        return /^(|#|#\/|#\/home(\.html)?(\?.*)?)$/.test(location.hash);
    }

    function isVideoPage() {
        return /^#\/video\b/.test(location.hash);
    }

    function back() {
        var overlay = openOverlay();
        if (overlay) {
            if (overlay.classList.contains('dialog') || overlay.classList.contains('actionSheet')) {
                history.back();
            } else {
                // React menus close on their own Escape handler.
                var t = overlay.contains(document.activeElement) ? document.activeElement : (overlay.querySelector('[tabindex]') || overlay);
                synthetic = true;
                try {
                    t.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape', keyCode: 27, which: 27, bubbles: true, cancelable: true }));
                } finally {
                    synthetic = false;
                }
            }
            return;
        }
        if (!isHome()) history.back();
    }

    // ---- keys ---------------------------------------------------------------

    var pendingDropdown = null;  // dropdown to open when Enter comes up
    var synthetic = false;   // a key we send ourselves (Escape to a menu, Space to a dropdown)
    var KEYS = { ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down' };

    function isTyping(el) {
        if (!el) return false;
        if (el.tagName === 'TEXTAREA' || el.isContentEditable) return true;
        return el.tagName === 'INPUT' && !/^(checkbox|radio|button|submit|range)$/i.test(el.type);
    }

    function consume(e) {
        e.preventDefault();
        e.stopImmediatePropagation();
    }

    function onKey(e) {
        if (!enabled || synthetic || e.defaultPrevented || e.ctrlKey || e.altKey || e.metaKey) return;
        stopRevealWatch();
        // The in-page dropdown (select-menu.js) handles its own keys.
        if (document.getElementById('_jselect')) return;
        var key = e.key;

        if (isVideoPage()) {
            // The player keeps its own keys (seek, pause); only leaving is ours.
            if (key === 'Escape' || key === 'Backspace') {
                consume(e);
                back();
            }
            return;
        }

        // Enter on a dropdown opens the app's own menu (select-menu.js), which
        // opens on Space; the Space is sent once Enter is released, so the
        // rest of that Enter cannot reach the menu.
        var el = document.activeElement;
        if ((key === 'Enter' || key === ' ') && el && el.tagName === 'SELECT') {
            if (key === 'Enter') {
                consume(e);
                pendingDropdown = el;
            }
            return;
        }

        if (isTyping(document.activeElement)) {
            if (key === 'Escape') {
                consume(e);
                document.activeElement.blur();
                return;
            }
            if (key !== 'ArrowUp' && key !== 'ArrowDown') return;
            document.activeElement.blur();
        }

        if (KEYS[key]) {
            consume(e);
            move(KEYS[key]);
        } else if (key === 'Enter' || key === ' ') {
            consume(e);
            activate();
        } else if (key === 'Escape' || key === 'Backspace') {
            consume(e);
            back();
        }
    }

    window.addEventListener('keydown', onKey, true);

    window.addEventListener('keyup', function (e) {
        var el = pendingDropdown;
        pendingDropdown = null;
        if (!el || e.key !== 'Enter' || document.activeElement !== el) return;
        consume(e);
        synthetic = true;
        try {
            el.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', code: 'Space', keyCode: 32, which: 32, bubbles: true, cancelable: true }));
        } finally {
            synthetic = false;
        }
    }, true);

    // A mouse in hand hides the highlight; the next arrow brings it back.
    window.addEventListener('mousemove', function (e) {
        if (enabled && (e.movementX || e.movementY)) {
            document.documentElement.classList.add('jrn-mouse');
            stopRevealWatch();
        }
    }, true);

    // ---- page changes -------------------------------------------------------

    // Keeps the highlight on screen while the page changes under it, driven
    // by the page's own mutations rather than by timers. After a navigation
    // it waits for the item you came back to, or for the page's content and
    // then takes the natural start. When a re-render or a closing menu takes
    // the highlighted element away, it moves to the item that opened the
    // menu, the same item re-rendered, or the start.
    var scheduled = false;
    var leftPage = null;   // the page shown when the address last changed

    function schedule() {
        if (scheduled || !enabled) return;
        scheduled = true;
        requestAnimationFrame(function () {
            scheduled = false;
            reconcile();
        });
    }

    // A row rebuilt on return (the home page reloads its sections) still has
    // no width when the item is restored, so it cannot scroll to it yet.
    // Watch the row and scroll once it has been laid out; any key press or
    // page change ends the watch.
    var revealWatch = null;

    function stopRevealWatch() {
        if (revealWatch) revealWatch.disconnect();
        revealWatch = null;
    }

    function keepRevealed(u) {
        stopRevealWatch();
        var s = scrollerOf(u);
        if (!s || typeof ResizeObserver !== 'function') return;
        revealWatch = new ResizeObserver(function () {
            if (current !== u || !u.isConnected) { stopRevealWatch(); return; }
            reveal(u);
        });
        revealWatch.observe(s);
        revealWatch.observe(s.querySelector('.scrollSlider') || s);
    }

    function reconcile() {
        if (!enabled || isVideoPage() || document.getElementById('_jselect')) return;
        // Until the page you left is hidden, the new one is not on screen yet
        // (or still animating in): wait, or the highlight would land on the
        // old page. A route that keeps the same page gets its highlight on
        // the next key press instead.
        if (leftPage) {
            if (leftPage.isConnected && isShown(leftPage)) return;
            leftPage = null;
        }
        var scope = currentScope();
        if (current && isShown(current) && scope.some(function (r) { return r && r.contains(current); })) return;
        var list = units(scope);
        if (!list.length) return;
        var rows = buildRows(list);
        if (openOverlay()) {
            // A menu or dialog just opened: start on its first item.
            setCurrent(initial(list), rows);
            return;
        }
        var wanted = pageMemory[location.hash];
        var found = wanted ? findDescribed(list, wanted) : null;
        if (found) {
            setCurrent(found, rows);
            keepRevealed(found);
        } else if (opener && list.indexOf(opener) >= 0) {
            setCurrent(opener, rows);
        } else if (!wanted && list.some(function (u) { return isContent(u) || u.matches('.btnPlay, .btnResume'); })) {
            setCurrent(initial(list), rows);
        } else if (current) {
            // Nothing to restore to yet; drop the stale highlight.
            current.removeAttribute('data-jrn-focus');
            current = null;
        }
    }

    // The router changes pages with pushState, which fires no event, so watch
    // the address instead.
    var lastHref = location.href;
    function routeChanged() {
        if (location.href === lastHref) return;
        lastHref = location.href;
        if (current) current.removeAttribute('data-jrn-focus');
        current = null;
        opener = null;
        stopRevealWatch();
        leftPage = activePage();
        schedule();
    }
    ['pushState', 'replaceState'].forEach(function (name) {
        var original = history[name];
        history[name] = function () {
            var result = original.apply(this, arguments);
            routeChanged();
            return result;
        };
    });
    window.addEventListener('popstate', routeChanged);
    window.addEventListener('hashchange', routeChanged);

    // ---- the switch ---------------------------------------------------------

    function apply() {
        document.documentElement.classList.toggle('jrn-on', enabled);
        var b = document.getElementById('jrn-toggle');
        if (b) {
            b.classList.toggle('on', enabled);
            b.title = enabled ? 'Remote control mode is on. Click to switch back to mouse and keyboard.' : 'Switch to remote control mode';
            b.setAttribute('aria-label', b.title);
            b.setAttribute('aria-pressed', enabled ? 'true' : 'false');
            b.firstChild.textContent = enabled ? 'settings_remote' : 'mouse';
        }
        if (!enabled && current) {
            current.removeAttribute('data-jrn-focus');
            current = null;
        }
    }

    function setEnabled(on) {
        enabled = on;
        try { localStorage.setItem(STORE_KEY, on ? '1' : '0'); } catch (e) { /* storage blocked */ }
        apply();
        if (on) schedule();
    }

    // The header is rendered by React and can be rebuilt at any time, so the
    // button is put back whenever it goes missing.
    function placeToggle() {
        if (document.getElementById('jrn-toggle')) return;
        var search = document.querySelector('header.MuiAppBar-root [aria-label="Search"], .skinHeader .headerSearchButton');
        if (!search || !search.parentElement) return;
        var b = document.createElement('button');
        b.id = 'jrn-toggle';
        b.type = 'button';
        var icon = document.createElement('span');
        icon.className = 'material-icons';
        icon.setAttribute('aria-hidden', 'true');
        b.appendChild(icon);
        b.addEventListener('click', function (e) {
            e.preventDefault();
            e.stopPropagation();
            setEnabled(!enabled);
        });
        search.parentElement.insertBefore(b, search);
        apply();
    }

    // Injected before the document exists, so wait for it.
    function start() {
        (document.head || document.documentElement).appendChild(style);
        new MutationObserver(function () {
            placeToggle();
            schedule();
        }).observe(document.documentElement, { childList: true, subtree: true });
        placeToggle();
        apply();
        schedule();
    }

    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
    else start();
})();
