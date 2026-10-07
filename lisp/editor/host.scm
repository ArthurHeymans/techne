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
(require "minibuffer.scm")
(require "buffers.scm")
(require "windows.scm")

(provide editor-press editor-click editor-message! session-quit?
         editor-panes editor-focus pane-status pane-display echo-line pane-layers cursor-shape
         editor-session-state editor-restore! editor-pane-places editor-take-request! editor-paged! editor-clipboard! editor-clipboard-out
         bound-keys editor-unsendable!)

(define (editor-press s key) (press s key))
(define (editor-click s view pos extend)
  (focus-view! s view)
  ((profile-click (sget s 'profile)) s pos extend))
;; The frontend paged VIEW: its caret goes to POS, extending a region.
(define (editor-paged! s view pos)
  (editor-click s view pos (or (sget s 'extend) (eq? (sget s 'mode) 'visual))))
(define (editor-message! s text) (message! s text))
(define (session-quit? s) (sget s 'quit))
;; Every keymap a key may be looked up in, those of the focused buffer
;; first.
(define (keymaps-to-send s)
  (reverse (fold (lambda (km acc) (if (memq km acc) acc (cons km acc)))
                 '()
                 (append (active-keymaps s 'chord) (active-keymaps s 'normal) (all-keymaps s (list minibuffer-map))))))

;; The key sequences bound in them, for the frontend to check which it can
;; send.
(define (bound-keys s)
  (delete-duplicates (append-map keymap-sequences (keymaps-to-send s))))

;; Bound keys the frontend cannot send: say which commands they leave out
;; of reach, and remember them.
;; Note the keys the terminal cannot send, unless a message (such as the
;; journal's recovery) is showing.
(define (editor-unsendable! s keys)
  (sset! s 'unsendable keys)
  (unless (or (null? keys) (sget s 'message))
    (message! s (string-append
                 "Keys this terminal cannot send: "
                 (string-join (map (lambda (k)
                                     (let ((b (key-binding (keymaps-to-send s) (kbd k))))
                                       (if (symbol? b) (string-append k " (" (symbol->string b) ")") k)))
                                   keys)
                              ", ")))))

;; The system clipboard, through the frontend: what another program put
;; there comes in as a kill; what is killed goes out.
(define (editor-clipboard! s text) (clipboard-in! s text))
(define (editor-clipboard-out s) (take-clipboard-out! s))

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
  (let* ((state (read (open-input-string text)))
         (field (lambda (k) (cdr (assq k state))))
         (open (lambda (path) (guard (e (#t #f)) (file-document path))))
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

;; The views shown, each read-only as its buffer's option says now.
(define (editor-panes s)
  (for-each (lambda (v)
              (let ((b (document-buffer (view-document v))))
                (when b (set-view-read-only! v (option b 'read-only)))))
            (session-panes s))
  (session-panes s))
(define (editor-pane-places s) (pane-places s))
(define (editor-focus s) (session-focus s))

(define (view-point v) (cadr (list-ref (view-ranges v) (view-primary v))))

;; File, modified mark, line, the modes on; in the focused pane, the modal
;; state too.
;; A mode's name as the mode line shows it: without "-mode".
(define (mode-label name)
  (let ((n (symbol->string name)))
    (if (string-suffix? "-mode" n) (substring n 0 (- (string-length n) 5)) n)))

;; File or buffer, modified mark, line, the major mode and the minor modes
;; on; in the focused pane, the modal state too.
(define (pane-status s view)
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

;; What the frontend draws beside VIEW's text, from its buffer's options:
;; (line-numbers eob-marker).
(define (pane-display s view)
  (let ((b (document-buffer (view-document view))))
    (list (option b 'line-numbers) (option b 'eob-marker))))

;; Keys waiting for the rest of their sequence, the search being typed,
;; and the message or open prompt.
(define (echo-line s)
  (let* ((prompt (and (eq? (profile-name (sget s 'profile)) 'modal) (modal-prompt s)))
         (pending (append (or (sget s 'prefix-keys) '()) (or (sget s 'pending) '()) (or (sget s 'mode-pending) '())))
         (parts (list (and (pair? pending) (string-append (string-join pending " ") "-"))
                      (and (sget s 'isearch) (string-append "I-search: " (cadr (sget s 'isearch))))
                      (or prompt (sget s 'message)))))
    (string-join (filter (lambda (x) x) parts) "  ")))

;; Highlights: the session's layers, and the matches of a search being
;; typed in the focused pane, as Emacs's isearch and lazy-highlight.
(define (pane-layers s view from to)
  (sort (append (buffer-layers (view-document view) from to) (search-highlights s view from to))
        (lambda (a b) (< (car a) (car b)))))

(define (search-highlights s view from to)
  (let ((needle (cond ((sget s 'isearch) (cadr (sget s 'isearch)))
                      ((eq? (sget s 'mode) 'search) (sget s 'search-input))
                      (else #f))))
    (if (and needle (not (string=? needle "")) (view=? view (pane-view s)))
        (let ((current (sget s 'isearch-match)))
          (map (lambda (m) (list (car m) (cadr m) (if (equal? m current) 'isearch 'lazy-highlight)))
               (search-all (view-document view) needle from to)))
        '())))

(define (cursor-shape s view)
  (if (and (view=? view (session-view s)) (memq (sget s 'mode) '(normal visual))) 'block 'bar))
