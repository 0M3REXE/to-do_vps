// Alpine components and glue for the whole app.
//
// Registered from a static file rather than inline `x-data="{}"` because the
// CSP forbids `unsafe-eval` and this build of Alpine (`@alpinejs/csp`,
// vendored as static/vendor/alpine.min.js) evaluates only a restricted,
// eval-free expression language. Directives stay readable in the templates;
// every non-trivial behavior lives here as a named component method.
//
// Rounds up everything the retired files did:
//   theme-toggle.js    → service-worker registration + theme flip below
//   password-toggle.js → togglePasswords: show/hide every password input
//   keyboard.js        → shortcuts, client-side search filter, focus restore
//   optimistic.js      → paint add/toggle/delete before the server round trip

document.addEventListener('alpine:init', function () {
  Alpine.data('app', function () {
    return {
      dark: false,
      search: '',
      restoreBusy: false,
      pendingListFocus: null,
      LEAVING_MS: 240, // matches --dur-base in site.css

      init: function () {
        // theme-init.js runs inline before paint; keep state in step with it.
        try {
          this.dark = localStorage.getItem('todo-theme') === 'dark';
        } catch (_) { /* Light default when storage is disabled. */ }

        if ('serviceWorker' in navigator) {
          navigator.serviceWorker.register('/service-worker.js').catch(function (error) {
            console.warn('Service worker registration failed', error);
          });
          navigator.serviceWorker.getRegistrations().then(function (registrations) {
            registrations.forEach(function (registration) {
              if (new URL(registration.scope).pathname === '/static/') registration.unregister();
            });
          });
        }

        this.bindHtmxGlue();
      },

      // --- Theme -----------------------------------------------------------

      toggleTheme: function () {
        this.dark = !this.dark;
        var html = document.documentElement;
        if (this.dark) html.setAttribute('data-theme', 'dark');
        else html.removeAttribute('data-theme');
        try {
          localStorage.setItem('todo-theme', this.dark ? 'dark' : 'light');
        } catch (_) { /* Theme still applies for this page. */ }
      },

      // --- Password fields ---------------------------------------------------

      // One method per form, not state per input: all password fields in a
      // form show and hide together, so the confirm field cannot drift.
      togglePasswords: function (event) {
        var button = event.currentTarget;
        var form = button.closest('form');
        if (!form) return;
        var fields = form.querySelectorAll('input[type="password"]');
        var willShow = fields.length > 0 && fields[0].type === 'password';
        fields.forEach(function (field) {
          field.type = willShow ? 'text' : 'password';
        });
        button.textContent = willShow ? 'Hide' : 'Show';
        button.setAttribute('aria-pressed', String(willShow));
      },

      // --- Shortcuts and search ----------------------------------------------

      shortcutAside: function (event) {
        return event.ctrlKey || event.metaKey || event.altKey ||
          this.isTyping(event);
      },
      isTyping: function (event) {
        var target = event.target;
        return target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' ||
          target.isContentEditable;
      },

      // '/' reveals the search box; modifiers are excluded here rather than by
      // .prevent so that typing a literal '/' inside an input is untouched.
      focusSearch: function (event) {
        if (event.ctrlKey || event.metaKey || event.altKey || this.isTyping(event)) return;
        event.preventDefault();
        var search = this.$refs.search;
        if (search) {
          search.focus();
          search.select();
        }
      },
      focusNewTodo: function (event) {
        if (event.ctrlKey || event.metaKey || event.altKey || this.isTyping(event)) return;
        event.preventDefault();
        if (this.$refs.newTodo) this.$refs.newTodo.focus();
      },
      escapeSearch: function () {
        var search = this.$refs.search;
        if (this.search && document.activeElement === search) {
          this.search = '';
          this.applyFilter();
          if (search) search.blur();
        }
      },
      clearSearch: function () {
        var search = this.$refs.search;
        if (!search) return;
        this.search = '';
        this.applyFilter();
        search.focus();
      },
      onSearchInput: function () {
        this.search = (this.$refs.search ? this.$refs.search.value : '');
        this.applyFilter();
      },

      applyFilter: function () {
        var q = this.search.toLowerCase().trim();
        var visible = 0;
        document.querySelectorAll('.todo-row[data-text]').forEach(function (row) {
          var text = (row.getAttribute('data-text') || '').toLowerCase();
          var match = q === '' || text.indexOf(q) !== -1;
          row.hidden = !match;
          if (match) visible++;
        });
        var emptyList = document.querySelector('[data-empty-list]');
        var noMatches = document.querySelector('[data-search-empty]');
        if (emptyList) emptyList.hidden = q !== '';
        if (noMatches) noMatches.hidden = q === '' || visible > 0;
      },

      // --- Optimistic todo mutations ------------------------------------------

      // The round trip (browser → app → libSQL → fragment) makes add,
      // toggle and delete feel laggy. These paint the change the instant htmx
      // issues a validated request, and the swapped server fragment replaces
      // everything with truth. hx-on attributes would need `unsafe-eval`, so
      // the glue is bound to these document listeners instead.

      sourceElement: function (event) {
        var ctx = event.detail && event.detail.ctx;
        var source = ctx && ctx.sourceElement;
        if (!source || !source.closest('#todo-panel')) return null;
        return source.matches('.todo-add, .todo-toggle, .todo-delete') ? source : null;
      },

      bumpCount: function (delta) {
        var count = document.getElementById('todo-count');
        if (!count) return;
        var open = count.textContent.match(/\d+/);
        if (open) {
          count.textContent = Math.max(0, Number(open[0]) + delta) + ' open';
        }
      },

      // Recovery path for every failure kind: refetch the authoritative page
      // and swap in its panel. Naturally discards a provisional add row or a
      // half-faded delete row too, since the whole panel is replaced.
      restorePanel: function () {
        var self = this;
        if (self.restoreBusy) return;
        self.restoreBusy = true;
        fetch('/app/', { credentials: 'same-origin' })
          .then(function (response) {
            if (!response.ok) throw new Error('restore got ' + response.status);
            return response.text();
          })
          .then(function (html) {
            var fresh = new DOMParser().parseFromString(html, 'text/html')
              .getElementById('todo-panel');
            var current = document.getElementById('todo-panel');
            if (fresh && current) current.replaceWith(fresh);
            self.applyFilter();
          })
          .catch(function () {})
          .then(function () {
            self.restoreBusy = false;
          });
      },

      // A new todo sorts last among active ones: the optimistic row goes
      // before the first completed row (or the end of the list). Cloned from a
      // live row for identical structure, then stripped of all htmx wiring so
      // its checkbox and delete button are inert until the real row arrives.
      insertProvisionalRow: function (text) {
        var list = document.getElementById('todo-list');
        if (!list) return;

        var donor = list.querySelector('.todo-row');
        var row;
        if (donor) {
          row = donor.cloneNode(true);
          row.classList.remove('todo-row--done', 'todo-row--leaving');
          row.setAttribute('data-optimistic', '');
          row.setAttribute('data-text', text);
          var cell = row.querySelector('.todo-text');
          if (cell) cell.textContent = text;
          var toggle = row.querySelector('.todo-toggle');
          if (toggle) {
            toggle.checked = false;
            toggle.removeAttribute('hx-post');
            toggle.setAttribute('aria-label', 'Mark complete: ' + text);
          }
          var del = row.querySelector('.todo-delete');
          if (del) {
            del.removeAttribute('hx-post');
            del.removeAttribute('hx-target');
            del.removeAttribute('hx-swap');
            del.removeAttribute('hx-confirm');
            del.setAttribute('aria-label', 'Delete ' + text);
          }
        } else {
          row = document.createElement('li');
          row.className = 'todo-row';
          row.setAttribute('data-optimistic', '');
          row.setAttribute('data-text', text);
          row.innerHTML = '<span class="todo-text"></span>';
          row.querySelector('.todo-text').textContent = text;
        }

        var active = list.querySelector('.todo-row--done');
        var marker = list.querySelector('[data-search-empty]');
        list.insertBefore(row, active || marker || null);

        var search = this.$refs.search;
        if (search && this.search) {
          this.applyFilter();
        } else {
          var empty = list.querySelector('[data-empty-list]');
          if (empty) empty.hidden = true;
        }
        this.bumpCount(+1);
      },

      bindHtmxGlue: function () {
        var self = this;

        // Keyboard-focus bookkeeping: which control was touched, so focus can
        // land near it again after the server's list-only swap. Registered
        // before the optimistic handler so the index is measured while the
        // row is still painted.
        document.addEventListener('htmx:before:request', function (event) {
          var ctx = event.detail && event.detail.ctx;
          var source = ctx && ctx.sourceElement;
          if (!source || !source.closest('.todo-row')) return;
          var row = source.closest('.todo-row');
          var visibleRows = Array.from(document.querySelectorAll('.todo-row'))
            .filter(function (item) { return !item.hidden; });
          self.pendingListFocus = {
            index: visibleRows.indexOf(row),
            request: source.getAttribute('hx-post'),
            action: source.classList.contains('todo-delete') ? 'delete' : 'toggle'
          };
        });

        // Fires only for requests htmx actually issues: after hx-confirm was
        // accepted and after native validation (empty input, over-length
        // text) passed — so an invalid todo is never painted optimistically.
        document.addEventListener('htmx:before:request', function (event) {
          var source = self.sourceElement(event);
          if (!source) return;

          if (source.classList.contains('todo-add')) {
            var input = source.querySelector('.todo-input');
            if (!input) return;
            self.insertProvisionalRow(input.value);
            input.value = '';
            return;
          }

          var row = source.closest('.todo-row');
          if (!row) return;

          if (source.classList.contains('todo-toggle')) {
            var done = source.checked;
            row.classList.toggle('todo-row--done', done);
            self.bumpCount(done ? -1 : +1);
            return;
          }

          if (source.classList.contains('todo-delete')) {
            var open = !row.classList.contains('todo-row--done');
            row.classList.add('todo-row--leaving');
            if (open) self.bumpCount(-1);
            // The response usually swaps the whole list before this runs; if
            // it is slow, the row never lingers as a half-faded husk.
            setTimeout(function () {
              row.hidden = true;
            }, self.LEAVING_MS);
          }
        });

        document.addEventListener('htmx:after:swap', function (event) {
          var ctx = event.detail && event.detail.ctx;
          var target = ctx && ctx.target;
          if (!target) return;
          if (target.id === 'todo-panel') {
            // The panel (search input included) was re-rendered; the typed
            // query outlives the swap because it lives in component state.
            var search = self.$refs.search;
            if (search) search.value = self.search;
            self.applyFilter();
          }
          if (target.id === 'todo-list') {
            self.applyFilter();
            if (self.pendingListFocus) {
              var rows = Array.from(document.querySelectorAll('.todo-row'));
              if (self.pendingListFocus.action === 'toggle') {
                rows.map(function (row) {
                  return row.querySelector('.todo-toggle');
                }).find(function (control) {
                  return control &&
                    control.getAttribute('hx-post') === self.pendingListFocus.request;
                })?.focus();
              } else {
                var visibleRows = rows.filter(function (row) {
                  return !row.hidden;
                });
                if (visibleRows.length) {
                  visibleRows[Math.min(
                    self.pendingListFocus.index,
                    visibleRows.length - 1
                  )].querySelector('.todo-delete')?.focus();
                } else if (self.$refs.search && self.search) {
                  self.$refs.search.focus();
                } else if (self.$refs.newTodo) {
                  self.$refs.newTodo.focus();
                }
              }
              self.pendingListFocus = null;
            }
          }
        });

        // Rollback. `htmx:error` covers network failures and aborted
        // requests (no response at all); `htmx:response:error` covers 4xx or
        // 5xx that DID get a body.
        document.addEventListener('htmx:error', function (event) {
          if (self.sourceElement(event)) self.restorePanel();
        });
        document.addEventListener('htmx:response:error', function (event) {
          var ctx = event.detail && event.detail.ctx;
          if (self.sourceElement(event) && ctx && ctx.response && ctx.response.status) {
            self.restorePanel();
          }
        });
      },
    };
  });
});
