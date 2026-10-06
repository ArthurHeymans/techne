;;; Editing sessions: one view driven by one key profile.
;;;
;;; A session is a property table, so profiles and packages keep their own
;;; state on it. It holds a view (techne-editor) and a profile. Keys are
;;; strings in Emacs notation: "a", "C-x", "M-f", "C-M-_", and the named keys
;;; "RET", "ESC", "DEL", "SPC", "TAB". `press` hands a key to the profile,
;;; which decides what it means.
;;;
;;; Commands are named procedures (session count) in one table; profiles bind
;;; keys to their names. Errors a command raises become the session's
;;; message, as in Emacs.

(provide make-session make-session-for-view sget sset! press press-keys type-text kbd
         session-view session-document
         define-command register-command! command command-names run-command message!
         make-keymap keymap? define-key! lookup-key keymap-sequences
         printable-key? key-char key-for-char
         make-profile profile? profile-name profile-click)

(define-record-type profile
  (make-profile name init key click)
  profile?
  (name profile-name)
  ;; (session) -> sets up the profile's state
  (init profile-init)
  ;; (session key) -> handles one key
  (key profile-key)
  ;; (session position extend?) -> handles a click the frontend resolved
  (click profile-click))

(define (sget s k) (hash-table-ref/default s k #f))
(define (sset! s k v) (hash-table-set! s k v))

(define (make-session doc actor profile)
  (make-session-for-view (make-view doc actor) profile))

(define (make-session-for-view view profile)
  (let ((s (make-hash-table)))
    (sset! s 'view view)
    (sset! s 'profile profile)
    ((profile-init profile) s)
    s))

(define (session-view s) (sget s 'view))
(define (session-document s) (view-document (sget s 'view)))

(define (press s key) ((profile-key (sget s 'profile)) s key))

(define (kbd keys) (string-split keys " "))

;; Press each key of a space-separated key description.
(define (press-keys s keys) (for-each (lambda (k) (press s k)) (kbd keys)))

;; Press the key of each character, as typing it would.
(define (type-text s text)
  (for-each (lambda (c) (press s (key-for-char c))) (string->list text)))

(define (key-for-char c)
  (case c
    ((#\space) "SPC")
    ((#\newline) "RET")
    ((#\tab) "TAB")
    (else (string c))))

(define (printable-key? key)
  (or (= (string-length key) 1) (member key '("SPC" "TAB"))))

(define (key-char key)
  (cond ((string=? key "SPC") #\space)
        ((string=? key "TAB") #\tab)
        (else (string-ref key 0))))

;;; Commands

(define %commands (make-hash-table))

(define (register-command! name doc proc)
  (hash-table-set! %commands name (cons doc proc)))

(define (command name)
  (let ((c (hash-table-ref/default %commands name #f)))
    (if c (cdr c) (error "no such command" name))))

(define (command-names) (hash-table-keys %commands))

;; The table calls the global binding, so redefining a command takes effect
;; at its next invocation.
(define-syntax define-command
  (syntax-rules ()
    ((_ (name s n) doc body ...)
     (begin
       (define (name s n) body ...)
       (register-command! 'name doc (lambda (s2 n2) (name s2 n2)))))))

(define (message! s text) (sset! s 'message text))

(define (error-text e)
  (cond ((error-object? e)
         (let ((irritants (error-object-irritants e)))
           (if (null? irritants)
               (error-object-message e)
               (string-append (error-object-message e) ": "
                              (string-join (map (lambda (x) (call-with-output-string (lambda (p) (display x p)))) irritants) " ")))))
        ((string? e) e)
        (else (call-with-output-string (lambda (p) (write e p))))))

;; Run a command with a count. What commands record for the next one (a
;; kill to append to, a goal column to keep) is moved from "now" to "last"
;; here.
(define (run-command s name n)
  (sset! s 'this-command name)
  (sset! s 'kill-now #f)
  (sset! s 'goal-now #f)
  (message! s #f)
  (guard (e (#t (message! s (error-text e))))
    ((command name) s n))
  (sset! s 'last-kill (sget s 'kill-now))
  (unless (sget s 'goal-now) (sset! s 'goal #f))
  (sset! s 'last-command name))

;;; Keymaps: key -> command name or keymap.

(define-record-type keymap
  (%make-keymap table)
  keymap?
  (table keymap-table))

(define (make-keymap) (%make-keymap (make-hash-table)))

(define (define-key! map keys binding)
  (let loop ((map map) (keys (kbd keys)))
    (if (null? (cdr keys))
        (hash-table-set! (keymap-table map) (car keys) binding)
        (let ((next (hash-table-ref/default (keymap-table map) (car keys) #f)))
          (if (keymap? next)
              (loop next (cdr keys))
              (let ((m (make-keymap)))
                (hash-table-set! (keymap-table map) (car keys) m)
                (loop m (cdr keys))))))))

;; The binding of a key sequence (a list of keys): a command name, a keymap
;; (a prefix), or #f.
(define (lookup-key map keys)
  (cond ((null? keys) map)
        ((not (keymap? map)) #f)
        (else (lookup-key (hash-table-ref/default (keymap-table map) (car keys) #f) (cdr keys)))))

;; Every key sequence bound in a keymap, as strings ("C-x C-s").
(define (keymap-sequences keymap)
  (let walk ((km keymap) (prefix '()))
    (apply append
           (map (lambda (key)
                  (let ((b (hash-table-ref/default (keymap-table km) key #f))
                        (keys (append prefix (list key))))
                    (if (keymap? b) (walk b keys) (list (string-join keys " ")))))
                (hash-table-keys (keymap-table km))))))
