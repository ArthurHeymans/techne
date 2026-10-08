;;; Editing sessions: views in panes, driven by one key profile.
;;;
;;; A session is a property table, so profiles and packages keep their own
;;; state on it. It holds its panes (views, techne-editor), the focused one
;;; and a profile (keymaps.scm). While the minibuffer is open, commands edit
;;; its input (`session-view`).

(require "keymaps.scm")

(provide make-session make-session-for-view sget sset! current-session set-current-session!
         session-view session-document session-panes session-focus set-session-panes! focus-view! view=?
         session-tree set-session-tree! tree-leaves split-pane! delete-pane! pane-places
         pane-view set-pane-view! document=?)

(define (sget s k) (hash-table-ref/default s k #f))
(define (sset! s k v) (hash-table-set! s k v))

(define (make-session doc actor profile)
  (make-session-for-view (make-view doc actor) profile))

(define (make-session-for-view view profile)
  (let ((s (make-hash-table)))
    (set-session-panes! s (list view) 0)
    (sset! s 'profile profile)
    ((profile-init profile) s)
    s))

;;; Panes: the views shown, in order, and the focused one; and how they
;;; tile the frame, a tree as Emacs's windows (EDITOR.md, section 9). A
;;; leaf is a pane's index; a split is (split dir parts), DIR `below` or
;;; `right`, PARTS a list of (share . tree), the shares summing to 1.

(define (session-panes s) (sget s 'panes))
(define (session-focus s) (sget s 'focus))

;; Set the panes. With as many as the tree has, it stays; else they are
;; stacked evenly.
(define (set-session-panes! s views focus)
  (sset! s 'panes views)
  (sset! s 'focus focus)
  (unless (and (sget s 'tree) (= (length (tree-leaves (sget s 'tree))) (length views)))
    (sset! s 'tree (stacked (length views)))))

(define (session-tree s) (sget s 'tree))
(define (set-session-tree! s tree) (sset! s 'tree tree))

(define (stacked n)
  (if (= n 1) 0 (list 'split 'below (map (lambda (i) (cons (/ 1.0 n) i)) (iota n)))))

(define (tree-leaves t)
  (if (integer? t) (list t) (append-map (lambda (p) (tree-leaves (cdr p))) (caddr t))))

;; The tree with each leaf K replaced by (F K): a number or a tree.
(define (tree-map t f)
  (if (integer? t) (f t) (list 'split (cadr t) (map (lambda (p) (cons (car p) (tree-map (cdr p) f))) (caddr t)))))

;; Split pane I in two, in DIR, the new pane (I + 1) after it with VIEW;
;; the panes after it move up one. The focus stays.
(define (split-pane! s dir view)
  (let ((i (session-focus s)) (panes (session-panes s)))
    (sset! s 'tree (tree-map (session-tree s)
                             (lambda (k)
                               (cond ((< k i) k)
                                     ((> k i) (+ k 1))
                                     (else (list 'split dir (list (cons 0.5 i) (cons 0.5 (+ i 1)))))))))
    (sset! s 'panes (append (take panes (+ i 1)) (list view) (drop panes (+ i 1))))))

;; Delete pane I: its space goes to the part before it, else after it,
;; whose nearest pane gets the focus.
(define (delete-pane! s i)
  (define (without t)
    (if (integer? t)
        t
        (let* ((parts (caddr t))
               (k (list-index (lambda (p) (equal? (cdr p) i)) parts)))
          (if k
              (let* ((gone (car (list-ref parts k)))
                     (to (if (> k 0) (- k 1) 1))
                     (heir (cdr (list-ref parts to)))
                     (rest (filter-map (lambda (j)
                                         (let ((p (list-ref parts j)))
                                           (cond ((= j k) #f)
                                                 ((= j to) (cons (+ (car p) gone) (cdr p)))
                                                 (else p))))
                                       (iota (length parts)))))
                (sset! s 'heir (if (> k 0) (last (tree-leaves heir)) (car (tree-leaves heir))))
                (if (null? (cdr rest)) (cdr (car rest)) (list 'split (cadr t) rest)))
              (list 'split (cadr t) (map (lambda (p) (cons (car p) (without (cdr p)))) parts))))))
  (let* ((tree (without (session-tree s)))
         (heir (sget s 'heir))
         (panes (session-panes s)))
    (sset! s 'tree (tree-map tree (lambda (k) (if (> k i) (- k 1) k))))
    (sset! s 'panes (append (take panes i) (drop panes (+ i 1))))
    (sset! s 'focus (if (> heir i) (- heir 1) heir))))

;; Each pane's place in the frame, in its order: (x y w h), fractions.
(define (pane-places s)
  (let ((places (make-vector (length (session-panes s)) #f)))
    (let place ((t (session-tree s)) (x 0.0) (y 0.0) (w 1.0) (h 1.0))
      (if (integer? t)
          (vector-set! places t (list x y w h))
          (let loop ((parts (caddr t)) (at 0.0))
            (unless (null? parts)
              (let ((share (car (car parts))))
                (if (eq? (cadr t) 'right)
                    (place (cdr (car parts)) (+ x (* w at)) y (* w share) h)
                    (place (cdr (car parts)) x (+ y (* h at)) w (* h share)))
                (loop (cdr parts) (+ at share)))))))
    (vector->list places)))
;; Views are the same when their ids are (the host may hand Lisp a new
;; handle to a view it already has).
(define (view=? a b) (= (view-id a) (view-id b)))

(define (focus-view! s view)
  (let loop ((vs (session-panes s)) (i 0))
    (cond ((null? vs) #f)
          ((view=? (car vs) view) (sset! s 'focus i) #t)
          (else (loop (cdr vs) (+ i 1))))))

;; Commands act on the focused pane's view and its document, or on the
;; minibuffer's input while it is open.
(define (session-view s) (or (sget s 'input-view) (pane-view s)))
(define (session-document s) (view-document (session-view s)))

;; The focused pane's view, and putting another in its place.
(define (pane-view s) (list-ref (session-panes s) (session-focus s)))
(define (set-pane-view! s view)
  (let ((panes (session-panes s)) (i (session-focus s)))
    (set-session-panes! s (append (take panes i) (list view) (drop panes (+ i 1))) i)))

;; Documents are the same when their ids are (each handle Lisp gets is a
;; new object).
(define (document=? a b) (= (document-id a) (document-id b)))

;; The session last started: the one code evaluated from the editor acts on.
(define %session #f)
(define (current-session) %session)
(define (set-current-session! s) (set! %session s))
