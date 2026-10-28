;;; What the runtime (techne-editor's runtime.rs) asks of the session: it
;;; hands it keys, clicks and the clipboard, and asks what to present (the
;;; panes, mode lines, highlights, the echo area, the cursor shape) and
;;; what to keep for coming back after a crash. The application (main.scm)
;;; provides these to the runtime.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "emacs.scm")
(require "modal.scm")
(require "files.scm")
(require "remote.scm")
(require "minibuffer.scm")
(require "syntax.scm")
(require "buffers.scm")
(require "windows.scm")

(provide editor-press editor-click editor-message! session-quit?
         editor-panes editor-focus pane-status pane-display echo-line pane-layers cursor-shape
         editor-session-state editor-restore! editor-pane-places editor-take-request! editor-paged! editor-clipboard! editor-clipboard-out
         bound-keys editor-unsendable! editor-take-violations!)

(define (editor-press s key)
  "Handle the key KEY in session S, as the frontend sends it."
  (press s key))
(define (editor-click s view pos extend)
  "Handle a click at POS in VIEW, which the frontend resolved, in S.
With EXTEND, the click extends the selection."
  (focus-view! s view)
  ((profile-click (sget s 'profile)) s pos extend))
;; The frontend paged VIEW: its caret goes to POS, extending a region.
(define (editor-paged! s view pos)
  "Put the caret of VIEW at POS in session S, where paging left it."
  (editor-click s view pos (or (sget s 'extend) (eq? (sget s 'mode) 'visual))))
(define (editor-message! s text)
  "Show TEXT, a message from the frontend, in session S."
  (message! s text))
(define (editor-take-violations! s)
  "Return and forget the violations raised in session S since last asked.
See `take-violations!`."
  (take-violations! s))
(define (session-quit? s)
  "Return #t if session S has been asked to end."
  (sget s 'quit))
;; Every keymap a key may be looked up in, those of the focused buffer
;; first.
(define (keymaps-to-send s)
  (reverse (fold (lambda (km acc) (if (memq km acc) acc (cons km acc)))
                 '()
                 (append (active-keymaps s 'chord) (active-keymaps s 'normal) (all-keymaps s (list minibuffer-map file-name-map isearch-map))))))

(define (bound-keys s)
  "Return every key sequence bound in session S.
The frontend checks which of them it can send."
  (delete-duplicates (append-map keymap-sequences (keymaps-to-send s))))

(define (editor-unsendable! s keys)
  "Note KEYS, bound keys the frontend cannot send, in session S.
Unless a message (such as the journal's recovery) is showing, say which
commands they leave out of reach."
  (sset! s 'unsendable keys)
  (unless (or (null? keys) (sget s 'message))
    (message! s (string-append
                 "Keys this terminal cannot send: "
                 (string-join (map (lambda (k)
                                     (let ((b (key-binding (keymaps-to-send s) (kbd k))))
                                       (if (symbol? b) (string-append k " (" (symbol->string b) ")") k)))
                                   keys)
                              ", ")))))

(define (editor-clipboard! s text)
  "Take TEXT, what the system clipboard has, as a kill in session S."
  (clipboard-in! s text))
(define (editor-clipboard-out s)
  "Return the newest kill of S for the system clipboard, or #f."
  (take-clipboard-out! s))

(define (state-name s)
  (case (sget s 'mode)
    ((insert) "INSERT")
    ((visual) "VISUAL")
    ((normal) "NORMAL")
    (else #f)))

;;; Coming back after a crash: the host keeps what `editor-session-state`
;;; last gave and hands it to the next runtime's `editor-restore!`. Files
;;; are opened again with their journals, so with their unsaved edits;
;;; generated buffers (lenses, views) are not kept.

(define (file-of d)
  (and (document-path d) (absolute-path (document-path d))))

(define (editor-session-state s)
  "Return what session S needs to come back after a crash, as a string.
That is its files, panes, carets, scroll and focus; the next runtime's
`editor-restore!` reads it."
  (let* ((kept (filter (lambda (v) (file-of (view-document v))) (session-panes s)))
         (focus (or (list-index (lambda (v) (view=? v (pane-view s))) kept) 0))
         (pane (lambda (v)
                 (let ((r (list-ref (view-ranges v) (view-primary v))))
                   (list (file-of (view-document v)) (car r) (cadr r) (view-scroll v))))))
    (call-with-output-string
     (lambda (p)
       (write `((buffers ,@(filter-map (lambda (b) (file-of (buffer-document b))) (buffer-list))) (panes ,@(map pane kept)) (focus ,focus)
                ;; The tiling, when every pane is kept.
                (tree ,(and (= (length kept) (length (session-panes s))) (session-tree s))))
              p)))))

(define (editor-restore! s text)
  "Bring back in session S the state TEXT `editor-session-state` gave.
Files are opened again with their journals, so with their unsaved
edits; files that are gone, and those on other machines, are left out."
  (let* ((state (read (open-input-string text)))
         (field (lambda (k) (cdr (assq k state))))
         ;; A file on another machine is not: connecting would hold the
         ;; restart up. Its unsaved edits wait in its journal for it to
         ;; be opened again.
         (open (lambda (path) (and (not (remote-path? path)) (guard (e (#t #f)) (file-document path)))))
         (view-at (lambda (d anchor head scroll)
                    (let ((v (make-view d "user")) (len (document-length d)))
                      (guard (e (#t #f)) (view-set-ranges! v (list (list (min anchor len) (min head len))) 0))
                      (guard (e (#t #f)) (view-set-scroll! v (min scroll len)))
                      (set-buffer-view! (add-buffer! d) v)
                      v)))
         (views (filter-map (lambda (p)
                              (let ((d (open (car p))))
                                (and d (view-at d (cadr p) (caddr p) (cadddr p)))))
                            (field 'panes))))
    (for-each (lambda (path) (let ((d (open path))) (when d (add-buffer! d)))) (reverse (field 'buffers)))
    (unless (null? views)
      (set-session-panes! s views (min (car (field 'focus)) (- (length views) 1)))
      (let ((tree (let ((t (assq 'tree state))) (and t (cadr t)))))
        (when (and tree (= (length (tree-leaves tree)) (length views)))
          (set-session-tree! s tree))))))

;;; What the frontend shows: panes, each with its mode line and the
;;; layers' highlights, and the echo area.

(define (editor-panes s)
  "Return the views the panes of session S show."
  (session-panes s))
(define (editor-pane-places s)
  "Return where each pane of session S is, as `pane-places` does."
  (pane-places s))
(define (editor-focus s)
  "Return the index of the focused pane of session S."
  (session-focus s))

(define (view-point v) (cadr (list-ref (view-ranges v) (view-primary v))))

;; File, modified mark, line, the modes on; in the focused pane, the modal
;; state too.
;; A mode's name as the mode line shows it: without "-mode".
(define (mode-label name)
  (let ((n (symbol->string name)))
    (if (string-suffix? "-mode" n) (substring n 0 (- (string-length n) 5)) n)))

(define (pane-status s view)
  "Return the mode line of VIEW in session S.
It says the file or buffer, the modified mark, the line, the major mode
and the minor modes on; in the focused pane, the modal state too."
  (let* ((d (view-document view))
         (b (document-buffer d))
         (focused (view=? view (session-view s)))
         (modes (if b (cons (buffer-mode b) (map (lambda (m) (mode-name (car m))) (buffer-minor-modes b))) '()))
         (parts (list (or (document-path d) (and b (buffer-name b)) "*scratch*")
                      (if (and (document-dirty? d) (not (and b (option b 'read-only)))) "[+]" #f)
                      (string-append "L" (number->string (line-number d (view-point view))))
                      (and focused (state-name s))
                      (and (pair? modes) (string-append "(" (string-join (map mode-label modes) " ") ")")))))
    (string-join (filter (lambda (x) x) parts) "  ")))

(define (pane-display s view)
  "Return what is drawn beside VIEW's text in session S, and its syntax.
That is (line-numbers eob-marker language), from its buffer's options:
LANGUAGE the syntax language to colour it by, or #f for none (never
without `syntax-highlight'; the mode's `syntax-language' first, then
the file's name)."
  (let* ((d (view-document view)) (b (document-buffer d)))
    (list (option b 'line-numbers) (option b 'eob-marker)
          (and (option b 'syntax-highlight) (buffer-language b)))))

(define (echo-line s)
  "Return the echo area of session S.
It shows the keys waiting for the rest of their sequence, the search
being typed, and the message."
  (let* ((pending (append (or (sget s 'prefix-keys) '()) (or (sget s 'pending) '()) (or (sget s 'mode-pending) '())))
         (parts (list (and (pair? pending) (string-append (string-join pending " ") "-"))
                      (and (sget s 'isearch) (string-append "I-search: " (cadr (sget s 'isearch))))
                      (sget s 'message))))
    (string-join (filter (lambda (x) x) parts) "  ")))

(define (pane-layers s view from to)
  "Return the highlights of VIEW from FROM to TO in session S.
Those are its buffer's layers and the matches of a search being typed
in the focused pane, as Emacs's isearch and lazy-highlight. Their
order is their precedence where they overlap: the matches last, over
the layers, which the runtime composes attribute by attribute."
  (append (buffer-layers (view-document view) from to)
          (search-highlights s view from to)))

;; How far past the scroll anchor matches are highlighted, in bytes.
(define search-highlight-span 16384)

(define (search-highlights s view from to)
  (let ((needle (cond ((sget s 'isearch) (cadr (sget s 'isearch)))
                      ((search-reading s) (minibuffer-input s))
                      (else #f))))
    (if (and needle (not (string=? needle "")) (view=? view (pane-view s)))
        (let ((current (let ((m (sget s 'isearch-match))) (and m (car m)))))
          (map (lambda (m) (list (car m) (cadr m) (if (eqv? (car m) current) 'isearch 'lazy-highlight)))
               ;; Over a few screens, not the whole window layers cover: a
               ;; letter typed matches thousands of times in 64 KB.
               ;; The span is in bytes: its end is rounded to a character.
               (let ((d (view-document view)))
                 (search-all d needle from
                             (char-start d (min to (+ from search-highlight-span) (document-length d)))))))
        '())))

(define (cursor-shape s view)
  "Return the shape of the caret of VIEW in session S: block or bar."
  (if (and (view=? view (session-view s)) (memq (sget s 'mode) '(normal visual))) 'block 'bar))
