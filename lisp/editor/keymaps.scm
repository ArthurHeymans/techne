;;; Keys, keymaps and input profiles. Keys are strings in Emacs notation:
;;; "a", "C-x", "M-f", "C-M-_", and the named keys "RET", "ESC", "DEL",
;;; "SPC", "TAB". A keymap maps a key to a command's name or to a keymap (a
;;; prefix). A profile decides what a key means (dispatch.scm hands it the
;;; keys): the Emacs profile's chords, the modal one's states.

(provide kbd make-keymap keymap? define-key! lookup-key keymap-keys keymap-sequences keymap-name name-prefix! prefix-bindings
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
  "Return the keymap of the profile P for the input STATE, or #f.
STATE is `chord` or `normal`."
  (let ((km (assq state (profile-keymaps p)))) (and km (cdr km))))

(define (kbd keys)
  "Return the keys of KEYS, a key description such as `\"C-x C-s\"`."
  (string-split keys " "))

(define (key-for-char c)
  "Return the key that types the character C."
  (case c
    ((#\space) "SPC")
    ((#\newline) "RET")
    ((#\tab) "TAB")
    (else (string c))))

(define (printable-key? key)
  "Return #t if KEY types a character when it is not bound."
  (or (= (string-length key) 1) (member key '("SPC" "TAB"))))

(define (key-char key)
  "Return the character KEY types."
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
  "Return a new keymap, without bindings."
  (letrec ((km (%make-keymap (make-hash-table) #f
                             (make-registry 'bindings
                                            #:changed (lambda (keys binding)
                                                        (if binding
                                                            (%define-key! km keys binding)
                                                            (%undefine-key! km (kbd keys))))))))
    km))

(define (name-prefix! map keys name)
  "Call the prefix KEYS of MAP NAME, as which-key shows it.
KEYS is a key description; it is an error if it is no prefix."
  (let ((s (current-scope)))
    (if (%scope-pending s)
        (%set-scope-pending! s (cons (lambda () (name-prefix! map keys name)) (%scope-pending s)))
        (let ((m (lookup-key map (kbd keys))))
          (if (keymap? m) (set-keymap-name! m name) (error "not a prefix" keys))))))

(define (prefix-bindings maps keys)
  "Return the bindings directly under the prefix KEYS in MAPS.
Each is (key . binding); the first map's binding of a key wins."
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

(define (define-key! map keys binding)
  "Bind KEYS, a key description, to BINDING in MAP.
A binding made in a scope other than the root is owned by it: shutting
the scope removes it, unless it has been rebound since. While a package
loads, the binding waits until the package is published."
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

(define (keymap-keys km)
  "Return the keys bound directly in the keymap KM, unsorted."
  (hash-table-keys (keymap-table km)))

(define (keymap-sequences km)
  "Return every key sequence bound in the keymap KM.
Each is a string as `kbd` reads it."
  (append-map (lambda (key)
                (let ((b (hash-table-ref/default (keymap-table km) key #f)))
                  (if (keymap? b)
                      (map (lambda (rest) (string-append key " " rest)) (keymap-sequences b))
                      (list key))))
              (hash-table-keys (keymap-table km))))

(define (lookup-key map keys)
  "Return the binding of KEYS, a list of keys, in MAP.
That is a command's name, a keymap (KEYS is a prefix), or #f."
  (cond ((null? keys) map)
        ((not (keymap? map)) #f)
        (else (lookup-key (hash-table-ref/default (keymap-table map) (car keys) #f) (cdr keys)))))
