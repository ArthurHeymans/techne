;;; Panes, as Emacs's windows: splitting, moving between them, closing
;;; them; and paging and recentering, which the frontend resolves.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")

(provide request-view! editor-take-request! next-screen-context-lines)

;;; Paging and recentering are visual: the command asks the frontend,
;;; which knows the screen; it answers with where it scrolled and where
;;; the caret goes (editor-paged!), or that there is nothing further.
;;; As Arthur's Emacs: two lines of context, the caret keeping its place
;;; on the screen, an error at either end.

(define next-screen-context-lines 2)

;; Requests of keys handled before the frontend answers add up: two pages
;; down are one of two screens.
(define (request-view! s request)
  (let ((old (sget s 'view-request)) (id (view-id (pane-view s))))
    (sset! s 'view-request
           (if (and old (= (car old) id) (eq? (cadr old) 'page) (eq? (car request) 'page))
               (list id 'page (+ (caddr old) (cadr request)) (caddr request))
               (cons id request)))))

(define (editor-take-request! s view)
  (let ((r (sget s 'view-request)))
    (and r (= (car r) (view-id view))
         (begin (sset! s 'view-request #f) (cdr r)))))


(define-command (scroll-up-command s n)
  "Show the next screen of text; the caret keeps its place on the screen."
  (request-view! s (list 'page (if (< n 0) -1.0 1.0) next-screen-context-lines)))

(define-command (scroll-down-command s n)
  "Show the previous screen of text; the caret keeps its place on the screen."
  (request-view! s (list 'page (if (< n 0) 1.0 -1.0) next-screen-context-lines)))

(define-command (scroll-half-down s n) "Show the next half screen." (request-view! s (list 'page 0.5 0)))
(define-command (scroll-half-up s n) "Show the previous half screen." (request-view! s (list 'page -0.5 0)))

(define-command (recenter-top-bottom s n)
  "Scroll the caret's line to the middle; again, to the top, then the bottom."
  (let ((at (if (eq? (sget s 'last-command) 'recenter-top-bottom)
                (case (sget s 'recentered) ((middle) 'top) ((top) 'bottom) (else 'middle))
                'middle)))
    (sset! s 'recentered at)
    (request-view! s (list 'recenter at))))

(define-command (recenter-middle s n) "Scroll the caret's line to the middle." (request-view! s (list 'recenter 'middle)))
(define-command (recenter-top s n) "Scroll the caret's line to the top." (request-view! s (list 'recenter 'top)))
(define-command (recenter-bottom s n) "Scroll the caret's line to the bottom." (request-view! s (list 'recenter 'bottom)))

;;; Panes.

(define-command (split-window-below s n)
  "Split the focused pane in two, one above the other, both showing its
buffer; the focus stays in the upper one."
  (split-pane! s 'below (view-split (pane-view s))))

(define-command (split-window-right s n)
  "Split the focused pane in two, side by side, both showing its buffer;
the focus stays in the left one."
  (split-pane! s 'right (view-split (pane-view s))))

(define-command (other-window s n)
  "Focus the next pane."
  (sset! s 'focus (modulo (+ (session-focus s) n) (length (session-panes s)))))

(define-command (delete-window s n)
  "Close the focused pane; its space goes to its neighbour, which gets the
focus."
  (let ((panes (session-panes s)) (i (session-focus s)))
    (if (= (length panes) 1)
        (message! s "Attempt to delete the sole window")
        (delete-pane! s i))))

(define-command (delete-other-windows s n)
  "Close every pane but the focused one."
  (set-session-panes! s (list (pane-view s)) 0))
