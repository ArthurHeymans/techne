;;; Transient menus, as Magit's: a prefix key shows, below the panes,
;;; the commands that can follow it in groups, and the switches and
;;; options that change what they do. One key runs a command with the
;;; arguments set; C-g or q leaves. The focused pane keeps the focus, so
;;; a command acts on what point is on, as it would without the menu.
;;;
;;; A menu is data: (title group ...), a group (heading entry ...), an
;;; entry one of
;;;
;;;   (suffix KEY DESCRIPTION ACTION [ENABLED?])  ACTION a command name or
;;;       (lambda (session args)); ENABLED? a predicate of the session;
;;;   (switch KEY DESCRIPTION ARGUMENT [ON?])     a flag such as "--all";
;;;   (option KEY DESCRIPTION ARGUMENT [VALUE])   ARGUMENT ends in "=", its
;;;       value is read in the minibuffer;
;;;   (choice KEY DESCRIPTION (ARGUMENT ...) [INDEX]) one of exclusive flags.
;;;
;;; KEY is typed key by key: "-a" is - then a. A suffix's command reads
;;; the arguments set with (transient-args s).

(require "session.scm")
(require "modes.scm")
(require "buffers.scm")
(require "windows.scm")
(require "views.scm")
(require "minibuffer.scm")
(require "dispatch.scm")

(provide transient! transient-args transient-arg? transient-value transient-open?
         read-char-choice!)

(define (append-map* f . lists) (apply append (apply map f lists)))

(define (entry-kind e) (car e))
(define (entry-key e) (cadr e))
(define (entry-description e) (caddr e))
(define (entry-ref e i default) (if (> (length e) i) (list-ref e i) default))

;; The keys of KEY: each character, or the words of a key description.
(define (key-sequence key)
  (cond ((string-contains key " ") (string-split key " "))
        ((member key '("TAB" "RET" "SPC" "DEL" "ESC")) (list key))
        ((and (> (string-length key) 2) (char=? (string-ref key 1) #\-) (memv (string-ref key 0) '(#\C #\M #\s)))
         (list key))
        (else (map string (string->list key)))))

(define (transient-args s)
  "Return the arguments the menu of session S set for the command it runs.
Outside a menu, no arguments: the commands' defaults."
  (or (sget s 'transient-args) '()))

(define (transient-arg? s argument)
  "Return #t when the transient menu of session S set the switch ARGUMENT.
ARGUMENT is as the menu writes it, \"--all\"."
  (and (member argument (transient-args s)) #t))

(define (transient-value s argument)
  "Return the value session S's menu gave ARGUMENT, or #f.
ARGUMENT ends in =, as \"--author=\"."
  (let ((found (find (lambda (a) (string-prefix? argument a)) (transient-args s))))
    (and found (substring found (string-length argument) (string-length found)))))

(define (transient-open? s)
  "Return #t while a transient menu is shown in session S."
  (and (sget s 'transient-menu) #t))

;; The arguments the entries set, in their order.
(define (current-args menu values)
  (append-map
   (lambda (group)
     (append-map (lambda (e)
                   (let ((v (hash-table-ref/default values (entry-key e) #f)))
                     (case (entry-kind e)
                       ((switch) (if v (list (cadddr e)) '()))
                       ((option) (if v (list (string-append (cadddr e) v)) '()))
                       ((choice) (if v (list (list-ref (cadddr e) v)) '()))
                       (else '()))))
                 (cdr group)))
   (cdr menu)))

(define (enabled? s e)
  (let ((p (and (eq? (entry-kind e) 'suffix) (entry-ref e 4 #f))))
    (or (not p) (p s))))

;; A group's lines, each a list of runs, and its width.
(define (group-lines s group values)
  (let ((lines
         (cons (list (list (car group) 'heading))
               (filter-map
                (lambda (e)
                  (let* ((key (entry-key e)) (v (hash-table-ref/default values key #f))
                         (pad (make-string (max 1 (- 3 (string-length key))) #\space)))
                    (case (entry-kind e)
                      ((suffix)
                       (and (enabled? s e)
                            (list " " (list key 'key) pad (entry-description e))))
                      ((switch)
                       (list " " (list key 'key) pad (entry-description e) " "
                             (list (string-append "(" (cadddr e) ")") (if v 'success 'shadow))))
                      ((option)
                       (list " " (list key 'key) pad (entry-description e) " "
                             (if v (list (string-append "(" (cadddr e) v ")") 'success)
                                 (list (string-append "(" (cadddr e) ")") 'shadow))))
                      ((choice)
                       (append (list " " (list key 'key) pad (entry-description e) " " (list "[" 'shadow))
                               (let loop ((choices (cadddr e)) (i 0) (out '()))
                                 (if (null? choices) (reverse out)
                                     (loop (cdr choices) (+ i 1)
                                           (cons (list (car choices) (if (eqv? v i) 'success 'shadow))
                                                 (if (= i 0) out (cons (list "|" 'shadow) out))))))
                               (list (list "]" 'shadow))))
                      (else #f))))
                (cdr group)))))
    (cons lines (apply max (map runs-width lines)))))

(define (runs-width runs)
  (apply + (map (lambda (r) (string-length (if (string? r) r (car r)))) runs)))

;; Groups side by side while they fit in WIDTH, a band; bands one below
;; another, a blank line between.
(define (menu-rows s menu values #:width [width 104])
  (let* ((groups (filter (lambda (g) (> (length (car g)) 1))
                         (map (lambda (g) (group-lines s g values)) (cdr menu))))
         (bands (let loop ((gs groups) (band '()) (used 0) (out '()))
                  (cond ((null? gs) (reverse (if (null? band) out (cons (reverse band) out))))
                        ((and (pair? band) (> (+ used (cdar gs)) width))
                         (loop gs '() 0 (cons (reverse band) out)))
                        (else (loop (cdr gs) (cons (car gs) band) (+ used (cdar gs) 4) out))))))
    (append
     (list (row (list (list (car menu) 'prompt) (list "   C-g quit" 'shadow)) #:key 'title))
     (append-map*
      (lambda (band i)
        (let ((height (apply max (map (lambda (g) (length (car g))) band))))
          (append
           (if (> i 0) (list (row "" #:key (list 'gap i))) '())
           (map (lambda (line)
                  (row (append-map* (lambda (g last?)
                                     (let ((runs (if (< line (length (car g))) (list-ref (car g) line) '())))
                                       (if last? runs
                                           (append runs (list (make-string (+ 4 (- (cdr g) (runs-width runs))) #\space))))))
                                   band (map (lambda (j) (= j (- (length band) 1))) (iota (length band))))
                       #:key (list 'line i line)))
                (iota height)))))
      bands (iota (length bands))))))

(define (new-menu-buffer!)
  (let loop ((i 1))
    (let ((name (if (= i 1) "*transient-menu*" (string-append "*transient-menu " (number->string i) "*"))))
      (if (buffer-named name) (loop (+ i 1))
          (make-generated-buffer! name (make-presentation) 'special-mode)))))

(define (render! s state)
  (let* ((b (hash-table-ref state 'buffer))
         (rows (menu-rows s (hash-table-ref state 'menu) (hash-table-ref state 'values))))
    (present! (buffer-document b) rows)
    (length rows)))

(define (close! s state)
  (let* ((v (hash-table-ref state 'view)) (focus (hash-table-ref state 'focus))
         (i (list-index (lambda (p) (view=? p v)) (session-panes s))))
    (sset! s 'transient-menu #f)
    (sset! s 'transient #f)
    (when i
      (delete-pane! s i)
      (sset! s 'focus (min (if (> focus i) (- focus 1) focus) (- (length (session-panes s)) 1))))
    (forget-buffer! (hash-table-ref state 'buffer))))

(define (run-suffix! s state e)
  (let ((args (current-args (hash-table-ref state 'menu) (hash-table-ref state 'values)))
        (action (cadddr e)))
    (close! s state)
    (sset! s 'transient-args args)
    (dynamic-wind
      (lambda () #f)
      (lambda () (if (procedure? action) (action s args) (run-command s action 1)))
      (lambda () (sset! s 'transient-args #f)))))

(define (entries state)
  (append-map cdr (cdr (hash-table-ref state 'menu))))

(define (toggle! s state e)
  (let ((values (hash-table-ref state 'values)) (key (entry-key e)))
    (case (entry-kind e)
      ((switch) (hash-table-set! values key (not (hash-table-ref/default values key #f))) (render! s state))
      ((choice)
       (let ((v (hash-table-ref/default values key #f)) (n (length (cadddr e))))
         (hash-table-set! values key (cond ((not v) 0) ((< (+ v 1) n) (+ v 1)) (else #f)))
         (render! s state)))
      ((option)
       (if (hash-table-ref/default values key #f)
           (begin (hash-table-set! values key #f) (render! s state))
           ;; Reading a value takes the keys; the menu comes back after.
           (begin
             (sset! s 'transient #f)
             (completing-read s (string-append (entry-description e) ": ") '() #:require-match #f
               #:accept (lambda (s c)
                          (let ((text (string-trim (candidate-text c))))
                            (hash-table-set! values key (and (not (string=? text "")) text))
                            (render! s state)
                            (sset! s 'transient (hash-table-ref state 'handler))))
               #:abort (lambda () (sset! s 'transient (hash-table-ref state 'handler))))))))))

(define (handle-key s state key)
  (let* ((typed (append (hash-table-ref/default state 'pending '()) (list key)))
         (matches (filter (lambda (e) (and (memq (entry-kind e) '(suffix switch option choice))
                                           (or (not (eq? (entry-kind e) 'suffix)) (enabled? s e))
                                           (let ((ks (key-sequence (entry-key e))))
                                             (and (<= (length typed) (length ks)) (equal? typed (take ks (length typed)))))))
                          (entries state)))
         (exact (find (lambda (e) (= (length (key-sequence (entry-key e))) (length typed))) matches)))
    (cond ((member key '("C-g" "ESC" "<escape>")) (close! s state) (message! s "Quit"))
          (exact
           (hash-table-set! state 'pending '())
           (if (eq? (entry-kind exact) 'suffix) (run-suffix! s state exact) (toggle! s state exact)))
          ((pair? matches) (hash-table-set! state 'pending typed)
                           (message! s (string-append (string-join typed "") "-")))
          ((and (null? (hash-table-ref/default state 'pending '())) (string=? key "q"))
           (close! s state))
          (else (hash-table-set! state 'pending '())
                (message! s (string-append (string-join typed " ") " is not in this menu; C-g quits"))))))

(define (transient! s menu #:values [initial '()])
  "Show MENU in session S until a key runs one of its commands or quits.
INITIAL is a list of (KEY . VALUE): switches on, options with values."
  (let ((open (sget s 'transient-menu)))
    (when open (close! s open)))
  (let* ((state (make-hash-table))
         (values (make-hash-table))
         (b (new-menu-buffer!))
         (handler (lambda (s key) (handle-key s state key))))
    (for-each (lambda (g)
                (for-each (lambda (e)
                            (let ((default (case (entry-kind e)
                                             ((switch option choice) (entry-ref e 4 #f))
                                             (else #f))))
                              (when default (hash-table-set! values (entry-key e) default))))
                          (cdr g)))
              (cdr menu))
    (for-each (lambda (kv) (hash-table-set! values (car kv) (cdr kv))) initial)
    (hash-table-set! state 'menu menu)
    (hash-table-set! state 'values values)
    (hash-table-set! state 'buffer b)
    (hash-table-set! state 'handler handler)
    (hash-table-set! state 'focus (session-focus s))
    (let* ((lines (render! s state))
           (panes (session-panes s))
           (v (make-view (buffer-document b) "transient-menu"))
           ;; The frame's height is the frontend's: a share that fits a
           ;; menu in a terminal of 24 lines and stays modest in a window.
           (share (min 0.6 (max 0.3 (/ (+ lines 2) 30.0)))))
      (hash-table-set! state 'view v)
      (sset! s 'panes (append panes (list v)))
      (set-session-tree! s (list 'split 'below (list (cons (- 1 share) (session-tree s)) (cons share (length panes))))))
    (sset! s 'transient-menu state)
    (sset! s 'transient handler)))

(define (read-char-choice! s prompt choices)
  "Ask PROMPT in the echo area of session S, answered by one key.
CHOICES are (KEY . THUNK); the thunk of the key typed runs. Quitting,
or a key not among them, answers nothing."
  (message! s prompt)
  (sset! s 'transient
         (lambda (s key)
           (sset! s 'transient #f)
           (let ((choice (assoc key choices)))
             (if choice ((cdr choice)) (message! s "Quit"))))))
