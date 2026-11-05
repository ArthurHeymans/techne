;;; Argument forms (doc/operations.md, "Rehearsing"): a row per argument,
;;; the contract's readers to change it, and the same `invoke` to run it.
;;; Opening a form captures context, not intent to execute. Old records
;;; supply only complete, non-sensitive data; every value is checked anew.

(require "session.scm")
(require "dispatch.scm")
(require "contracts.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "views.scm")
(require "operations.scm")

(provide command-form open-command-form! again-operation
         argument-form? form-command form-arguments form-problems form-set-argument! form-clear-argument! form-invoke!)

;; Each opening owns its buffer: forms of the same command in different
;; attachments (or a second form while a reader waits) cannot replace it.
(define %next-form 0)

(define-record-type argument-form
  (make-argument-form command arguments source code contract parent principal notes last version id)
  argument-form?
  (id form-id)
  (command form-command)
  (arguments form-arguments set-form-arguments!)
  (source form-source)
  (code form-code set-form-code!)
  (contract form-contract set-form-contract!)
  (parent form-parent)
  (principal form-principal)
  (notes form-notes set-form-notes!)
  (last form-last set-form-last!)
  (version form-version set-form-version!))

(define (written v) (call-with-output-string (lambda (p) (write v p))))
(define (entries f)
  (contract-entries (or (command-contract (form-command f)) (error "The command no longer has a contract"))))

;; Valid arguments, in order, and (name . why) for those invalid. A later
;; schema only sees validated predecessors, never another's invalid value.
(define (checked f)
  (let loop ((es (entries f)) (valid (dict)) (problems '()))
    (if (null? es)
        (list valid (reverse problems))
        (let* ((e (car es)) (key (car e)) (given (dict-contains? (form-arguments f) key)))
          (cond ((not given)
                 (loop (cdr es) valid (if (memq #:optional (cddr e)) problems (cons (cons key "missing") problems))))
                (else
                 (let ((result (guard (x (#t (cons #f (error-text x))))
                                 (cons #t (contract-conform-argument (form-command f) key (dict-ref (form-arguments f) key) valid)))))
                   (if (car result)
                       (loop (cdr es) (dict-set valid key (cdr result)) problems)
                       (loop (cdr es) valid (cons (cons key (cdr result)) problems))))))))))

(define (form-problems f)
  "Return (argument . explanation) for every invalid argument of form F.
Dependent schemas see only valid preceding arguments. Nothing runs."
  (cadr (checked f)))

(define (form-set-argument! f key value)
  "Set argument KEY of form F to VALUE; return its validation problems.
The value stays editable even when invalid; all later, dependent
arguments are checked again. No command or effect is executed."
  (unless (assq key (entries f)) (error "Not an argument" key))
  (set-form-arguments! f (dict-set (form-arguments f) key value))
  (set-form-version! f (+ 1 (form-version f)))
  (form-problems f))

(define (form-clear-argument! f key)
  "Clear argument KEY in form F, returning its validation problems.
An optional argument is then omitted; a required one needs input."
  (unless (assq key (entries f)) (error "Not an argument" key))
  (set-form-arguments! f (dict-delete (form-arguments f) key))
  (set-form-version! f (+ 1 (form-version f)))
  (form-problems f))

;; The form's pane must never become the command's context. Refuse if
;; its original view was closed, rather than silently using another one.
(define (at-source s f thunk)
  (let* ((old (session-focus s))
         (index (list-index (lambda (v) (view=? v (form-source f))) (session-panes s))))
    (unless index (error "The form's source pane was closed; open a new form"))
    (dynamic-wind (lambda () (sset! s 'focus index)) thunk
                  (lambda () (when (< old (length (session-panes s))) (sset! s 'focus old))))))

(define (form-invoke! s f #:mode [mode 'apply])
  "Invoke form F's command in S, in MODE apply or stage.
Revalidate all arguments in its original pane, then use `invoke`.
A changed body or contract is shown for review before it can run.
Return the command's result; its operation is linked to the old one
when the form was opened by `again-operation`."
  (unless (memq mode '(apply stage)) (error "A form runs in apply or stage mode"))
  (let* ((name (form-command f)) (contract (or (command-contract name) (error "The command no longer has a contract"))))
    (when (or (not (eqv? (form-code f) (command-code name))) (not (= (form-contract f) (contract-hash contract))))
      (set-form-code! f (command-code name))
      (set-form-contract! f (contract-hash contract))
      (set-form-notes! f '("The command changed while this form was open. Review its arguments and run again."))
      (error "The command changed; review the form before running"))
    (let ((state (checked f)))
      (unless (null? (cadr state)) (error "Arguments need attention" (map car (cadr state))))
      (at-source s f
        (lambda ()
          (invoke s name (car state) #:mode mode #:principal (form-principal f) #:parent (form-parent f)
                  #:started (lambda (r) (set-form-last! f r))))))))

(define (argument-key i) (if (< i 10) (number->string (modulo (+ i 1) 10)) "e"))

(define (form-rows f)
  (let ((problems (form-problems f)))
    (append
     (list (row "command  " (list (list (symbol->string (form-command f)) 'function)) #:key 'command
                #:target (target 'command (form-command f)))
           (row "keys  " "1–0 / e edit   o clear   RET apply   C-c C-s stage   q quit" #:key 'keys))
     (map (lambda (note) (row "review  " (list (list note 'warning)))) (form-notes f))
     (map (lambda (e i)
            (let* ((key (car e)) (raw (form-arguments f)) (problem (assq key problems))
                   (secret (memq #:sensitive (cddr e)))
                   (shown (cond ((not (dict-contains? raw key)) (if (memq #:optional (cddr e)) "<omitted>" "<missing>"))
                                (secret "<sensitive>")
                                (else (written (->data (dict-ref raw key))))))
                   (doc (memq #:doc (cddr e))))
              (row (string-append "[" (argument-key i) "] " (symbol->string key) "  ")
                   (list (list shown (if problem 'warning 'string)))
                   (if doc (string-append "  — " (cadr doc)) "")
                   (if problem (list "  " (list (cdr problem) 'error)) "")
                   #:key (list 'argument key) #:target (target 'command-argument (cons f key)))))
          (entries f) (iota (length (entries f))))
     (if (form-last f)
         (let* ((r (form-last f)) (op (record-operation r)))
           (list (row "last run  " (string-append (number->string (operation-serial op)) " "
                                                  (if (record-pending? r) "pending" (symbol->string (operation-outcome op)))
                                                  (if (operation-staged? op) " (staged; inspect to review proposals)" ""))
                      #:key 'last #:target (target 'operation r))))
         '()))))

(define-mode command-form-mode
  "Arguments of a command, checked against its live contract.
Number keys edit an argument; \\[form-edit] edits the row at point.
\\[form-apply] invokes it; \\[form-stage] stages it without editing sources."
  #:parent 'rows-mode
  #:keys '(("RET" form-apply) ("C-c C-c" form-apply) ("C-c C-s" form-stage) ("e" form-edit) ("o" form-clear) ("q" quit-window)
           ("1" form-edit-1) ("2" form-edit-2) ("3" form-edit-3) ("4" form-edit-4) ("5" form-edit-5)
           ("6" form-edit-6) ("7" form-edit-7) ("8" form-edit-8) ("9" form-edit-9) ("0" form-edit-0))
  #:normal '(("RET" form-apply) ("C-c C-c" form-apply) ("C-c C-s" form-stage) ("e" form-edit) ("o" form-clear) ("q" quit-window)
             ("1" form-edit-1) ("2" form-edit-2) ("3" form-edit-3) ("4" form-edit-4) ("5" form-edit-5)
             ("6" form-edit-6) ("7" form-edit-7) ("8" form-edit-8) ("9" form-edit-9) ("0" form-edit-0)))

(define (show-form! s f)
  (show-view! s (string-append "*arguments: " (symbol->string (form-command f)) " " (number->string (form-id f)) "*")
              (lambda (s) (form-rows f)) #:mode 'command-form-mode #:data f))

(define (open-command-form! s name #:record [r #f])
  "Show command NAME's argument form in S; return the form.
Without record R capture the source's context and declared defaults.
With a completed operation's R, fill only its complete,
non-sensitive arguments, noting changed code or contract. Nothing runs."
  (let* ((c (or (command-contract name) (error "This command has no contract" name)))
         (args (if r
                   (fold (lambda (a d)
                           (if (or (memq (car a) (record-unavailable-arguments r))
                                   (not (memq (car a) (contract-params c)))) d (dict-set d (car a) (cdr a))))
                         (dict) (or (record-arguments r) '()))
                   (command-context-arguments s name)))
         (source (if (and r (record-document r))
                     (or (find (lambda (v) (= (document-id (view-document v)) (record-document r))) (session-panes s))
                         (error "The operation's source is not shown; show it before running again"))
                     (pane-view s)))
         (notes (if r
                    (append (if (eqv? (command-code name) (record-code r)) '() '("The command's body was redefined since this operation."))
                            (if (eqv? (contract-hash c) (record-contract r)) '() '("The command's contract changed; check every argument."))
                            (if (null? (record-unavailable-arguments r)) '() '("Arguments not kept must be entered again when required.")))
                    '()))
         (f (make-argument-form name args source (command-code name) (contract-hash c)
                                (and r (record-operation r)) (current-principal) notes #f 0
                                (begin (set! %next-form (+ %next-form 1)) %next-form))))
    (when (and r (record-pending? r)) (error "The operation is still pending"))
    (show-form! s f)
    f))

(define-command (command-form s n)
  "Choose a command with a contract and edit its arguments before running.
Context values are captured from this pane. Nothing executes until
\\[form-apply] or \\[form-stage] in its argument form."
  (completing-read s "Arguments of command: "
                   (map (lambda (name) (candidate (symbol->string name) #:annotation (car (string-split (command-doc name) "\n"))))
                        (sort (filter command-contract (command-names))
                              (lambda (a b) (string<? (symbol->string a) (symbol->string b)))))
                   #:accept (lambda (s c) (open-command-form! s (string->symbol (candidate-text c))))))

(define-action operation (again-operation s r)
  "Review this operation's arguments in a form, then run it again.
References are resolved and checked anew; redacted or shortened values
must be entered again. Opaque commands cannot be replayed."
  (unless (record-arguments r) (error "This operation has no arguments to replay"))
  (open-command-form! s (record-command r) #:record r))

(define (here s)
  (let ((f (view-data (current-buffer s))))
    (if (argument-form? f) f (error "Not an argument form"))))

(define (edit-argument! s f key)
  (let* ((state (checked f)) (es (entries f))
         (before (take es (or (list-index (lambda (e) (eq? (car e) key)) es) (error "Not an argument" key))))
         (args (fold (lambda (e d)
                       (let ((k (car e))) (if (dict-contains? (car state) k) (dict-set d k (dict-ref (car state) k)) d)))
                     (dict) before))
         (version (form-version f)))
    (at-source s f
      (lambda ()
        (read-command-argument! s (form-command f) key args
          (lambda (value)
            (unless (= version (form-version f)) (error "The form changed while this argument was being read"))
            (form-set-argument! f key value)
            (show-form! s f))
          #:initial (if (dict-contains? (form-arguments f) key) (list (dict-ref (form-arguments f) key)) '()))))))

(define-action command-argument (edit-command-argument s pair)
  "Edit this argument using its command's reader."
  (edit-argument! s (car pair) (cdr pair)))

(define-command (form-edit s n)
  "Edit the argument at point using its contract's reader."
  (let ((t (target-at (doc s) (point s))))
    (unless (and t (eq? (target-kind t) 'command-argument)) (error "No argument on this row"))
    (edit-command-argument s (target-value t))))

(define-command (form-clear s n)
  "Clear the argument at point: omit it if optional, else require input."
  (let ((t (target-at (doc s) (point s))))
    (unless (and t (eq? (target-kind t) 'command-argument)) (error "No argument on this row"))
    (let ((pair (target-value t)))
      (form-clear-argument! (car pair) (cdr pair))
      (show-form! s (car pair)))))

(define (edit-index! s i)
  (let* ((f (here s)) (es (entries f)))
    (unless (< i (length es)) (error "No argument with this key"))
    (edit-argument! s f (car (list-ref es i)))))

(define-syntax argument-key-command
  (syntax-rules ()
    ((_ name i)
     (define-command (name s n) "Edit the numbered argument in this command form." (edit-index! s i)))))
(argument-key-command form-edit-1 0)
(argument-key-command form-edit-2 1)
(argument-key-command form-edit-3 2)
(argument-key-command form-edit-4 3)
(argument-key-command form-edit-5 4)
(argument-key-command form-edit-6 5)
(argument-key-command form-edit-7 6)
(argument-key-command form-edit-8 7)
(argument-key-command form-edit-9 8)
(argument-key-command form-edit-0 9)

(define (submit! s mode)
  (let ((f (here s)))
    (guard (e (#t (show-form! s f) (raise e)))
      (form-invoke! s f #:mode mode)
      (show-form! s f))))

(define-command (form-apply s n)
  "Validate and apply this form's command in its original pane."
  (submit! s 'apply))

(define-command (form-stage s n)
  "Validate and stage this form's command: edits become proposals.
Inspect its last run to review them; settings and external effects are refused."
  (submit! s 'stage))
