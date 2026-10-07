;;; Checking documentation, as Emacs's checkdoc and package-lint check
;;; theirs: every name a module provides has a docstring, and every
;;; command, action, mode, option and hook one too, each following the
;;; convention (runtime/TECHNE-VM.md, "Docstrings"); a docstring's
;;; \\[command] names a command; a key is bound to a command or to a
;;; named prefix. `checkdoc` shows what is wrong in a view, each problem
;;; at its definition; the editor's tests keep the list of what is wrong
;;; in the editor itself from growing.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "emacs.scm")
(require "modal.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "completion.scm")
(require "views.scm")

(provide module-documentation-problems editor-documentation-problems documentation-problems
         problem-module problem-subject problem-name problem-text problem-location)

;; A problem: in MODULE (a module's name, or `editor` for its registries),
;; what kind of thing SUBJECT (`procedure`, `command`, `keymap`...) is
;; wrong, its NAME, the TEXT saying what to change and the LOCATION of its
;; definition, (file line column) or #f.
(define-record-type problem
  (%make-problem module subject name text location)
  problem?
  (module problem-module)
  (subject problem-subject)
  (name problem-name)
  (text problem-text)
  (location problem-location))

;; The module the problems being made are in.
(define %module 'editor)
(define (make-problem subject name text location) (%make-problem %module subject name text location))

(define (internal? name) (string-prefix? "%" (symbol->string name)))

;; The problems of DOC, the docstring of a SUBJECT named NAME, with
;; PARAMS: none, or the docstring's own and the commands it names that
;; do not exist.
(define (doc-problems subject name doc params location)
  (let ((rule (case subject ((procedure built-in) 'procedure) ((command action) 'command) (else 'other))))
    (map (lambda (text) (make-problem subject name text location))
         (if (string? doc)
             (append (docstring-problems doc rule params)
                     (filter-map (lambda (c)
                                   (and (not (memq c (command-names)))
                                        (string-append "\\[" (symbol->string c) "] names no command.")))
                                 (docstring-key-references doc)))
             (list "Document it with a docstring.")))))

(define (field alist key) (let ((e (assq key alist))) (and e (cdr e))))

;; Commands and actions are checked as such, with their registries.
(define (registered? name)
  (or (memq name (command-names)) (memq name (map action-name (all-actions)))))

;; The problems of the binding NAME in MODULE.
(define (binding-problems module name)
  (let ((d (binding-description name module)))
    (if (not d)
        (list (make-problem 'binding name "It is provided but not defined." #f))
        (let ((kind (field d 'kind)))
          (doc-problems (cond ((eq? kind 'procedure) 'procedure)
                              ((eq? kind (string->symbol "built-in procedure")) 'built-in)
                              ((eq? kind 'macro) 'macro)
                              (else 'variable))
                        name (field d 'doc) (or (field d 'params) '()) (field d 'location))))))

(define (module-documentation-problems module)
  "Return the problems of the documentation of what MODULE provides.
MODULE is named as `eval` names one; for the root module, which provides
nothing, the problems of every name it defines but internal ones (%...).
A name is checked where it is defined, not where it is provided again;
commands and actions are checked with the editor's registries."
  (let* ((own (module-definitions module))
         (names (filter (lambda (n) (memq n own))
                        (or (module-exports module) (if (equal? module "root") own '())))))
    (set! %module module)
    (let ((problems (append-map (lambda (name) (binding-problems module name))
                                (remove (lambda (n) (or (internal? n) (registered? n))) (delete-duplicates names)))))
      (set! %module 'editor)
      problems)))

(define (by-name items name-of)
  (sort items (lambda (a b) (string<? (symbol->string (name-of a)) (symbol->string (name-of b))))))

;; Commands: their own docstring, on the procedure the command runs.
(define (command-problems)
  (append-map (lambda (name)
                (doc-problems 'command name (command-doc name) '() (procedure-location (command name))))
              (by-name (command-names) (lambda (n) n))))

(define (registry-problems subject names doc-of)
  (append-map (lambda (name) (doc-problems subject name (doc-of name) '() #f)) (by-name names (lambda (n) n))))

(define (action-problems)
  (append-map (lambda (a) (doc-problems 'action (action-name a) (action-doc a) '() #f))
              (by-name (all-actions) action-name)))

;; Keymaps: each binding a command.
(define (keymap-problems label km)
  (filter-map (lambda (seq)
                (let ((b (lookup-key km (kbd seq))))
                  (and (symbol? b) (not (memq b (command-names)))
                       (make-problem 'keymap label (string-append seq " is bound to " (symbol->string b) ", which is no command.") #f))))
              (keymap-sequences km)))

;; Prefixes of the profiles' keymaps, as which-key shows them.
(define (prefix-problems label km)
  (let walk ((km km) (keys '()))
    (append-map (lambda (key)
                  (let ((b (lookup-key km (list key))))
                    (if (keymap? b)
                        (append (if (keymap-name b)
                                    '()
                                    (list (make-problem 'keymap label
                                                        (string-append "Name the prefix " (string-join (append keys (list key)) " ")
                                                                       " with `name-prefix!`.")
                                                        #f)))
                                (walk b (append keys (list key))))
                        '())))
                (keymap-keys km))))

(define (all-mode-keymaps)
  (append-map (lambda (name) (list (cons name (mode-map name)) (cons name (mode-map name #:state 'normal))))
              (by-name (mode-names) (lambda (n) n))))

(define (editor-documentation-problems)
  "Return the problems of the documentation of the editor's registries.
Those are its commands, actions, modes, options and hooks, and its
keymaps: a key bound to something that is no command, a prefix of a
profile's keymap without a name."
  (let ((profiles (list (cons 'emacs emacs-map) (cons 'modal modal-map))))
    (append (command-problems)
            (action-problems)
            (registry-problems 'mode (mode-names) (lambda (n) (mode-doc (find-mode n))))
            (registry-problems 'option (option-names) (lambda (n) (option-doc (find-option n))))
            (registry-problems 'hook (hook-names) hook-doc)
            (append-map (lambda (e) (keymap-problems (car e) (cdr e)))
                        (append profiles
                                (list (cons 'minibuffer minibuffer-map) (cons 'completion completion-map))
                                (all-mode-keymaps)))
            (append-map (lambda (e) (prefix-problems (car e) (cdr e))) profiles))))

(define (documentation-problems [modules (loaded-modules)])
  "Return the problems of the documentation of MODULES and the editor's.
MODULES are named as `eval` names them, all those loaded by default."
  (append (append-map module-documentation-problems modules) (editor-documentation-problems)))

(define (problem-rows problems)
  (map (lambda (p)
         (let ((where (problem-location p)))
           (row (string-append (symbol->string (problem-subject p)) " " (symbol->string (problem-name p)))
                (problem-text p)
                #:target (and where (target 'location (file-location (car where) (cadr where)))))))
       problems))

(define-command (checkdoc s n)
  "Show the problems of the documentation of the code loaded.
Each is shown at its definition, where it has one: RET goes there."
  (show-view! s "*checkdoc*" (lambda (s) (problem-rows (documentation-problems)))))
