;;; Keys, keymaps and input profiles. Keys are strings in Emacs notation:
;;; "a", "C-x", "M-f", "C-M-_", and the named keys "RET", "ESC", "DEL",
;;; "SPC", "TAB". A keymap maps a key to a command's name or to a keymap (a
;;; prefix). A profile decides what a key means (dispatch.scm hands it the
;;; keys): the Emacs profile's chords, the modal one's states.

(provide kbd make-keymap keymap? define-key! lookup-key keymap-sequences keymap-name name-prefix! prefix-bindings
         printable-key? key-char key-for-char
         make-profile profile? profile-name profile-init profile-key profile-click profile-keymap)

(define-record-type profile
  (make-profile name init key click keymaps)
  profile?
  (name profile-name)
  ;; (session) -> sets up the profile's state
  (init profile-init)
  ;; (session key) -> handles one key
  (key profile-key)
  ;; (session position extend?) -> handles a click the frontend resolved
  (click profile-click)
  ;; Its own keymap for each input state it has: an alist of (state .
  ;; keymap), the states `chord` and `normal`.
  (keymaps profile-keymaps))

(define (profile-keymap p state)
  (let ((km (assq state (profile-keymaps p)))) (and km (cdr km))))

(define (kbd keys) (string-split keys " "))

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

;;; Keymaps: key -> command name or keymap.

(define-record-type keymap
  (%make-keymap table name bindings)
  keymap?
  (table keymap-table)
  ;; What a prefix map is for, as which-key shows it ("+file").
  (name keymap-name set-keymap-name!)
  ;; Who bound what: a registry of key descriptions, whose entries in
  ;; effect `table` holds.
  (bindings keymap-bindings))

(define (make-keymap)
  (letrec ((km (%make-keymap (make-hash-table) #f
                             (make-registry 'bindings
                                            #:changed (lambda (keys binding)
                                                        (if binding
                                                            (%define-key! km keys binding)
                                                            (%undefine-key! km (kbd keys))))))))
    km))

;; Name the prefix KEYS of MAP (a key description), for which-key.
(define (name-prefix! map keys name)
  (let ((m (lookup-key map (kbd keys))))
    (if (keymap? m) (set-keymap-name! m name) (error "not a prefix" keys))))

;; The bindings directly under the prefix KEYS in MAPS, the first map's
;; first: a list of (key . binding).
(define (prefix-bindings maps keys)
  (fold (lambda (map acc)
          (let ((m (lookup-key map keys)))
            (if (keymap? m)
                (append acc (filter-map (lambda (k)
                                          (and (not (assoc k acc))
                                               (cons k (hash-table-ref/default (keymap-table m) k #f))))
                                        (hash-table-keys (keymap-table m))))
                acc)))
        '()
        maps))

;; Bind KEYS (a key description) to BINDING in MAP, owned by the current
;; scope: shutting it removes the binding, and the one it shadowed is in
;; effect again (a package's key over your own). While a package loads, the
;; binding waits until the package is published.
(define (define-key! map keys binding)
  (registry-add! (keymap-bindings map) (string-join (kbd keys) " ") binding))

(define (%undefine-key! map keys)
  (let ((m (lookup-key map (reverse (cdr (reverse keys))))))
    (when (keymap? m) (hash-table-delete! (keymap-table m) (last keys)))))

(define (%define-key! map keys binding)
  (let loop ((map map) (keys (kbd keys)))
    (if (null? (cdr keys))
        (hash-table-set! (keymap-table map) (car keys) binding)
        (let ((next (hash-table-ref/default (keymap-table map) (car keys) #f)))
          (if (keymap? next)
              (loop next (cdr keys))
              (let ((m (make-keymap)))
                (hash-table-set! (keymap-table map) (car keys) m)
                (loop m (cdr keys))))))))

;; Every key sequence bound in a keymap, each a string as `kbd` reads it.
(define (keymap-sequences km)
  (append-map (lambda (key)
                (let ((b (hash-table-ref/default (keymap-table km) key #f)))
                  (if (keymap? b)
                      (map (lambda (rest) (string-append key " " rest)) (keymap-sequences b))
                      (list key))))
              (hash-table-keys (keymap-table km))))

;; The binding of a key sequence (a list of keys): a command name, a keymap
;; (a prefix), or #f.
(define (lookup-key map keys)
  (cond ((null? keys) map)
        ((not (keymap? map)) #f)
        (else (lookup-key (hash-table-ref/default (keymap-table map) (car keys) #f) (cdr keys)))))
