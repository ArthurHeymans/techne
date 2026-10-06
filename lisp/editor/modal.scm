;;; The modal (Vim-like) profile: normal, insert, visual and search states;
;;; counts; the operators d, c and y with a motion, a text object, or doubled
;;; for whole lines; and `.` for simple changes (EDITOR.md, section 4).
;;;
;;; The normal-mode cursor sits on a character: it is a caret before that
;;; character, kept off the end of a non-empty line. A visual selection is
;;; stored as the half-open range it covers; the cursor is the last character
;;; it covers on the side being moved.
;;;
;;; `.` repeats the intent of the last change (the operator with its motion
;;; or object and count, then what was typed in insert mode), not the keys
;;; that made it.

(require "session.scm")
(require "commands.scm")

(provide modal-profile modal-prompt)

(define (state s) (or (sget s 'mode) 'normal))

;; The cursor: the head in normal mode, the tracked cursor in visual mode.
(define (cursor s) (if (eq? (state s) 'visual) (sget s 'cursor) (point s)))

(define (set-cursor! s p)
  (if (eq? (state s) 'visual)
      (let ((a (sget s 'visual-anchor)) (d (doc s)))
        (sset! s 'cursor p)
        (view-set-ranges! (session-view s)
                          (list (if (>= p a) (list a (next-grapheme d p)) (list (next-grapheme d a) p)))
                          0))
      (view-set-ranges! (session-view s) (list (list p p)) 0)))

;; Keep the normal-mode cursor on a character.
(define (clamp! s)
  (when (eq? (state s) 'normal)
    (let* ((d (doc s)) (p (point s)))
      (when (and (= p (line-end d p)) (> p (line-start d p)))
        (set-cursor! s (prev-grapheme d p))))))

;; Run an action as a command: errors become the message, and vertical
;; motion keeps its column. Vim's register does not join kills.
(define-command (modal-action s n) "Run the pending modal action." ((sget s 'action) s))
(define (act! s proc)
  (sset! s 'action proc)
  (run-command s 'modal-action 1)
  (sset! s 'last-kill #f))

;;; Motions, limited to the line where Vim limits them.

(define in-line-backward
  (make-motion (lambda (s p n) (max (line-start (doc s) p) ((motion-move char-backward) s p n))) 'exclusive))
(define in-line-forward
  (make-motion (lambda (s p n) (min (line-end (doc s) p) ((motion-move char-forward) s p n))) 'exclusive))
(define first-line (make-motion (lambda (s p n) 0) 'linewise))
(define last-line
  (make-motion (lambda (s p n) (line-start (doc s) (document-length (doc s)))) 'linewise))

(define motions
  (list (cons "h" in-line-backward) (cons "l" in-line-forward)
        (cons "w" vim-word-forward) (cons "b" vim-word-backward) (cons "e" vim-word-end)
        (cons "0" line-beginning) (cons "$" line-ending)
        (cons "j" line-next) (cons "k" line-previous)
        (cons "G" last-line) (cons "gg" first-line)))

(define (motion-for key) (let ((m (assoc key motions))) (and m (cdr m))))

;; An operator's extent from a motion; `dw` stops at the end of the line, and
;; `cw` changes only to the end of the word, as in Vim.
(define (extent-for s op m n)
  (let* ((d (doc s))
         (p (cursor s))
         (word-change (and (eq? op 'c) (eq? m vim-word-forward) (< p (document-length d))
                           (not (char-whitespace? (string-ref (document-substring d p (next-grapheme d p)) 0)))))
         (e (motion-extent s (if word-change vim-word-end m) p n)))
    (if (and (eq? m vim-word-forward) (not word-change) (> (cadr e) (line-end d p)) (< p (line-end d p)))
        (list (car e) (line-end d p) #f)
        e)))

;;; Operators

;; Apply an operator to the extents `extents-of` gives at the cursor.
(define (operate! s op extents-of)
  (let ((extents (extents-of s)))
    (case op
      ((y)
       (let ((e (car extents)))
         (kill-save! s (document-substring (doc s) (car e) (cadr e)) (caddr e) #f)
         (set-cursor! s (car e))))
      ((d) (delete-extents! s extents 'forward 'new))
      ((c)
       ;; A changed line keeps its line break.
       (let ((extents (map (lambda (e) (if (caddr e) (list (car e) (line-end (doc s) (car e)) #f) e)) extents)))
         (delete-extents! s extents 'forward 'new)
         (enter-insert! s #t))))))

;; Record a change for `.`: `redo` makes it again at the cursor. A change
;; that ends in insert mode is complete when insert mode is left.
(define (change! s redo)
  (unless (sget s 'replaying)
    (if (eq? (state s) 'insert)
        (begin (sset! s 'pending-change redo) (sset! s 'insert-keys '()))
        (sset! s 'last-change redo))))

(define (operator! s op extents-of)
  (let ((redo (lambda (s) (operate! s op extents-of))))
    (act! s redo)
    (unless (eq? op 'y) (change! s redo))))

;;; Insert mode

(define (enter-insert! s extend)
  (sset! s 'mode 'insert)
  ;; After an operator the typed text joins its undo unit.
  (sset! s 'insert-extend extend)
  (sset! s 'insert-keys '()))

(define (insert-edit! s proc)
  (proc (if (sget s 'insert-extend) 'extend 'new))
  (sset! s 'insert-extend #t))

(define (insert-key s key)
  (cond ((member key '("ESC" "C-g")) (leave-insert! s))
        (else
         (unless (sget s 'replaying) (sset! s 'insert-keys (cons key (sget s 'insert-keys))))
         (act! s (lambda (s)
                   (cond ((string=? key "RET") (insert-edit! s (lambda (g) (insert-text! s "\n" g))))
                         ((string=? key "DEL")
                          (insert-edit! s (lambda (g) (delete-extents! s (list (motion-extent s char-backward (point s) 1)) #f g))))
                         ((printable-key? key) (insert-edit! s (lambda (g) (insert-text! s (string (key-char key)) g))))
                         (else (error "not bound in insert mode" key))))))))

(define (leave-insert! s)
  (sset! s 'mode 'normal)
  (let ((d (doc s)) (p (point s)))
    (when (> p (line-start d p)) (set-cursor! s (prev-grapheme d p))))
  (unless (sget s 'replaying)
    (let ((start (sget s 'pending-change)) (keys (reverse (sget s 'insert-keys))))
      (when start
        (sset! s 'last-change
               (lambda (s)
                 (start s)
                 (for-each (lambda (k) (insert-key s k)) keys)
                 (leave-insert! s)))
        (sset! s 'pending-change #f))))
  (clamp! s))

;; Start inserting where `place` (session -> position) says; `opener` makes
;; a new line first, for o and O.
(define (insert-at! s place opener)
  (let ((redo (lambda (s)
                (let ((p (place s)))
                  (set-cursor! s p)
                  (enter-insert! s #f)
                  (when opener (opener s))))))
    (act! s redo)
    (change! s redo)))

(define (open-below s)
  (insert-edit! s (lambda (g) (insert-text! s "\n" g))))

(define (open-above s)
  (let ((p (point s)))
    (insert-edit! s (lambda (g) (insert-text! s "\n" g)))
    (set-cursor! s p)))

;;; Normal and visual mode

(define (reset-pending! s)
  (sset! s 'count #f)
  (sset! s 'op #f)
  (sset! s 'op-count #f)
  (sset! s 'prefix #f))

(define (total-count s)
  (* (or (sget s 'op-count) 1) (or (sget s 'count) 1)))

(define (digit-key? key) (and (= (string-length key) 1) (char-numeric? (string-ref key 0))))

(define (visual-extents s)
  (map (lambda (r) (list (min (car r) (cadr r)) (max (car r) (cadr r)) #f)) (ranges s)))

(define (leave-visual! s)
  (let ((c (sget s 'cursor)))
    (sset! s 'mode 'normal)
    (set-cursor! s c)))

(define (motion-key s m n)
  (let ((op (sget s 'op)))
    (reset-pending! s)
    (if op
        (operator! s op (lambda (s) (list (extent-for s op m n))))
        (act! s (lambda (s) (set-cursor! s ((motion-move m) s (cursor s) n)))))))

(define (paste! s after)
  (let ((redo
         (lambda (s)
           (let* ((k (yank-text s)) (text (car k)) (d (doc s)) (p (point s)))
             (if (cdr k)
                 ;; Whole lines go on their own line.
                 (let* ((span (line-span d p))
                        (at (if after (cadr span) (car span)))
                        (ends-file (and after (= at (document-length d))
                                        (not (string=? (document-substring d (prev-grapheme d at) at) "\n")))))
                   (if ends-file
                       (edit! s (list (list at at (string-append "\n" (substring text 0 (- (string-length text) 1))))) 'new)
                       (edit! s (list (list at at text)) 'new))
                   (set-cursor! s (if ends-file (+ at 1) at)))
                 (let ((at (if (and after (< p (line-end d p))) (next-grapheme d p) p)))
                   ;; From a caret at `at` the edit moves it past the text.
                   (set-cursor! s at)
                   (edit! s (list (list at at text)) 'new)
                   (set-cursor! s (prev-grapheme (doc s) (point s)))))))))
    (act! s redo)
    (change! s redo)))

(define (search-forward! s needle)
  (act! s (lambda (s)
            (let ((d (doc s)))
              (set-cursor! s (car (search! s needle (next-grapheme d (cursor s)) #t)))))))

;; Keys that need neither an operator nor a motion.
(define (simple-key s key n)
  (cond
   ((string=? key "x") (operator! s 'd (lambda (s) (list (motion-extent s in-line-forward (cursor s) n)))))
   ((string=? key "X") (operator! s 'd (lambda (s) (list (motion-extent s in-line-backward (cursor s) n)))))
   ((string=? key "D") (operator! s 'd (lambda (s) (list (motion-extent s line-ending (cursor s) n)))))
   ((string=? key "C") (operator! s 'c (lambda (s) (list (motion-extent s line-ending (cursor s) n)))))
   ((string=? key "i") (insert-at! s point #f))
   ((string=? key "a") (insert-at! s (lambda (s) (let ((p (point s))) (min (line-end (doc s) p) (next-grapheme (doc s) p)))) #f))
   ((string=? key "I") (insert-at! s (lambda (s) (line-start (doc s) (point s))) #f))
   ((string=? key "A") (insert-at! s (lambda (s) (line-end (doc s) (point s))) #f))
   ((string=? key "o") (insert-at! s (lambda (s) (line-end (doc s) (point s))) open-below))
   ((string=? key "O") (insert-at! s (lambda (s) (line-start (doc s) (point s))) open-above))
   ((string=? key "p") (paste! s #t))
   ((string=? key "P") (paste! s #f))
   ((string=? key "u") (run-command s 'undo n))
   ((string=? key "C-r") (run-command s 'redo n))
   ((string=? key "v")
    (sset! s 'visual-anchor (point s))
    (sset! s 'mode 'visual)
    (set-cursor! s (point s)))
   ((string=? key "/") (sset! s 'mode 'search) (sset! s 'search-input ""))
   ((string=? key ":") (sset! s 'mode 'ex) (sset! s 'search-input ""))
   ((string=? key "n")
    (if (sget s 'search-needle) (search-forward! s (sget s 'search-needle)) (message! s "no previous search")))
   ((string=? key ".")
    (let ((redo (sget s 'last-change)))
      (if redo
          (begin (sset! s 'replaying #t)
                 (act! s redo)
                 (sset! s 'replaying #f))
          (message! s "no change to repeat"))))
   (else (message! s (string-append key " is undefined")))))

(define (normal-key s key)
  (let ((op (sget s 'op)) (prefix (sget s 'prefix)) (visual (eq? (state s) 'visual)))
    (cond
     ((member key '("ESC" "C-g"))
      (if (or op prefix (sget s 'count)) (reset-pending! s) (when visual (leave-visual! s))))
     ;; Text objects after an operator: iw, aw.
     ((member prefix '("i" "a"))
      (let ((around (string=? prefix "a")) (n (total-count s)))
        (reset-pending! s)
        (if (string=? key "w")
            (operator! s op (lambda (s)
                              (let ((r (word-object (doc s) (cursor s) 'vim around)))
                                (list (list (car r) (cadr r) #f)))))
            (message! s (string-append prefix key " is not a text object")))))
     ((equal? prefix "g")
      (sset! s 'prefix #f)
      (if (string=? key "g") (motion-key s first-line (total-count s)) (reset-pending! s)))
     ((and (digit-key? key) (or (sget s 'count) (not (string=? key "0"))))
      (sset! s 'count (+ (* 10 (or (sget s 'count) 0)) (- (char->integer (string-ref key 0)) 48))))
     ((and op (member key '("i" "a"))) (sset! s 'prefix key))
     ((string=? key "g") (sset! s 'prefix "g"))
     ;; dd, cc, yy: whole lines.
     ((and op (string=? key (symbol->string op)))
      (let ((n (total-count s)))
        (reset-pending! s)
        (operator! s op (lambda (s) (list (motion-extent s line-next (cursor s) (- n 1)))))))
     ((motion-for key) => (lambda (m) (motion-key s m (total-count s))))
     ((member key '("d" "c" "y"))
      (let ((op (string->symbol key)))
        (if visual
            (begin (reset-pending! s)
                   (sset! s 'mode 'normal)
                   (act! s (lambda (s) (operate! s op (lambda (s) (visual-extents s))))))
            (begin (sset! s 'op op)
                   (sset! s 'op-count (sget s 'count))
                   (sset! s 'count #f)))))
     ((and visual (string=? key "x")) (normal-key s "d"))
     (else
      (let ((n (total-count s)))
        (reset-pending! s)
        (simple-key s key n)))))
  (clamp! s))

;; Ex commands: the few a file needs. They run the named commands, which
;; the application defines (main.scm).
(define ex-commands '(("w" save-buffer) ("q" quit) ("wq" save-buffer quit) ("x" save-buffer quit)))

(define (run-ex! s input)
  (let ((c (assoc input ex-commands)))
    (if c
        (for-each (lambda (name) (run-command s name 1)) (cdr c))
        (message! s (string-append "Not an editor command: " input)))))

;; The search and ex prompts read a line.
(define (search-key s key)
  (let ((input (sget s 'search-input)) (ex (eq? (state s) 'ex)))
    (cond ((member key '("ESC" "C-g")) (sset! s 'mode 'normal))
          ((and ex (string=? key "RET"))
           (sset! s 'mode 'normal)
           (run-ex! s input))
          ((string=? key "RET")
           (sset! s 'mode 'normal)
           (sset! s 'search-needle input)
           (search-forward! s input))
          ((string=? key "DEL")
           (if (string=? input "")
               (sset! s 'mode 'normal)
               (sset! s 'search-input (substring input 0 (- (string-length input) 1)))))
          ((printable-key? key) (sset! s 'search-input (string-append input (string (key-char key)))))
          (else #f))))

(define (modal-key s key)
  (case (state s)
    ((insert) (insert-key s key))
    ((search ex) (search-key s key))
    (else (normal-key s key))))

;; A click moves the cursor there; with extend, a visual selection runs to it.
(define (modal-click s pos extend)
  (when (and extend (not (eq? (state s) 'visual)))
    (sset! s 'visual-anchor (point s))
    (sset! s 'mode 'visual))
  (reset-pending! s)
  (set-cursor! s pos)
  (clamp! s))

;; What the prompt line shows, if a prompt is open.
(define (modal-prompt s)
  (case (state s)
    ((search) (string-append "/" (sget s 'search-input)))
    ((ex) (string-append ":" (sget s 'search-input)))
    (else #f)))

(define modal-profile
  (make-profile 'modal
                (lambda (s) (sset! s 'mode 'normal) (reset-pending! s))
                modal-key
                modal-click))
