;;; Faces: named styles for text (EDITOR.md, section 3).
;;;
;;; A face is a name for a partial set of attributes: foreground and
;;; background colours, weight, slant, an underline, and a few more. An
;;; attribute a face does not set is taken from what it inherits, then
;;; from what is under it where it is drawn, in the end from the
;;; `default' face — which sets the two colours; anything even it
;;; leaves open is the frontend's own normal (upright, undecorated).
;;; Layers and runs name faces;
;;; the runtime composes the faces over a place attribute by attribute
;;; and the frontends draw the result with what they have (a terminal
;;; has no heights, say), so what a name means is decided here, once.
;;;
;;; A face is declared with `define-face', usually by the package whose
;;; text it styles, and set with `set-face!', by a theme or the user.
;;; A theme is a package (load-package) whose top level is `set-palette!'
;;; and `set-face!' calls in its own scope: loading it again, or
;;; unloading it, is the next theme or the one before. `load-theme' is
;;; that, with the name it goes by.
;;;
;;; Colours are written as a palette name (a symbol), as "#rrggbb", or
;;; as (colour ansi), giving also the ANSI colour, 0 to 15, for
;;; frontends with only those. No other list is a colour: a list of
;;; more than two, headed by a symbol, is kept for colour expressions,
;;; should a theme ever want to compute one (a blend), so what is
;;; written today stays unambiguous. The palette maps names to colours, and
;;; names to names, so a face spec says `mauve' and a light theme is a
;;; palette away. The palette's `ansi-0' to `ansi-15' are what a
;;; terminal buffer's own colours will be drawn with.
;;;
;;; Names are dotted, the more specific after the dot, as tree-sitter's
;;; captures are: a name that is not declared stands for its longest
;;; declared prefix, and inherits from it unless it says its own
;;; #:inherit. So a package styles its directories `link.directory'
;;; and is themed wherever `link' is, until someone targets it;
;;; declaring it only adds its documentation and its own defaults.
;;;
;;; The runtime asks for `face-table' when `face-generation' changes:
;;; every declared face, resolved through its inheritance and the
;;; palette to numbers. Nothing here runs while text is drawn.

(require "../schema.scm")

(provide define-face register-face! find-face face-names face-doc
         set-face! unset-face! explain-face face-attributes
         set-palette! palette-color load-theme unload-theme
         face-table face-generation)

;;; The table's generation, bumped whenever a definition, a setting or
;;; a palette entry in effect changes. The runtime compares it before
;;; asking for `face-table' again.
(define %face-generation 0)
(define (face-changed! key value) (set! %face-generation (+ 1 %face-generation)))

(define (face-generation)
  "Return how often the faces in effect have changed.
The runtime takes `face-table' again when this has."
  %face-generation)

;;; Declarations

(define-record-type face-declaration
  (%make-face name doc inherit spec)
  face-declaration?
  (name face-name)
  (doc face-doc)
  ;; The face it inherits from: a symbol, #f for none, or () for its
  ;; longest declared dotted prefix.
  (inherit face-inherit)
  ;; Its own attributes, an alist (attribute . value) as given.
  (spec face-declared-spec))

(define %faces (make-registry 'faces #:changed face-changed!))
;; Settings, by face name: an alist of attributes, as `set-face!' gives.
;; Read as a whole stack (merged attribute by attribute), so a change
;; under the setting in effect changes a face too: #:stacked.
(define %face-settings (make-registry 'face-settings #:changed face-changed! #:stacked #t))
;; Palette entries, by name: a colour as it is written.
(define %palette (make-registry 'palette #:changed face-changed!))

(define (find-face name)
  "Return the declaration of the face NAME, or #f if there is none."
  (registry-ref %faces name))
(define (face-names)
  "Return the names of the faces declared."
  (registry-keys %faces))

;; A colour as it is written: a palette name, "#rrggbb", or (colour ansi).
;; The six digits are checked as digits: string->number alone would
;; take a sign ("#-12345") and give the face table a bad channel.
(define (written-colour? v)
  (define (hex? c) (or (char<=? #\0 c #\9) (char<=? #\a c #\f) (char<=? #\A c #\F)))
  (cond ((symbol? v) #t)
        ((string? v) (and (= (string-length v) 7) (char=? (string-ref v 0) #\#)
                          (let loop ((i 1)) (or (= i 7) (and (hex? (string-ref v i)) (loop (+ i 1)))))))
        ((and (pair? v) (pair? (cdr v)) (null? (cddr v)))
         (and (written-colour? (car v)) (exact-integer? (cadr v)) (<= 0 (cadr v) 15)))
        (else #f)))

(define (check-attributes who attrs)
  (for-each (lambda (e)
              (let ((a (car e)) (v (cdr e)))
                (unless (eq? v #f)
                  (case a
                    ((fg bg) (unless (written-colour? v) (error (string-append who ": not a colour, for " (symbol->string a) ":") v)))
                    ((weight) (unless (valid? '(one-of light normal bold) v) (error (string-append who ": not a weight:") v)))
                    ((slant) (unless (valid? '(one-of normal italic) v) (error (string-append who ": not a slant:") v)))
                    ((underline) (unless (or (eq? v #t) (valid? '(one-of line curl dotted dashed double) v)
                                             (and (pair? v) (pair? (cdr v)) (null? (cddr v))
                                                  (valid? '(one-of line curl dotted dashed double) (car v)) (written-colour? (cadr v))))
                                   (error (string-append who ": not an underline:") v)))
                    ((strike extend) (unless (boolean? v) (error (string-append who ": not a boolean, for " (symbol->string a) ":") v)))
                    ((family) (unless (or (symbol? v) (string? v)) (error (string-append who ": not a family:") v)))
                    ((height) (unless (and (real? v) (> v 0)) (error (string-append who ": not a height:") v)))
                    (else (error (string-append who ": no such face attribute:") a))))))
            attrs))

;; The attributes not () among the keyword values, as an alist. () is
;; the keyword parameters' "not given": an attribute set to #f among
;; them is given, and clears the attribute where it is merged over.
(define (attributes-of fg bg weight slant underline strike extend family height)
  (filter (lambda (e) (not (null? (cdr e))))
          (list (cons 'fg fg) (cons 'bg bg) (cons 'weight weight) (cons 'slant slant) (cons 'underline underline)
                (cons 'strike strike) (cons 'extend extend) (cons 'family family) (cons 'height height))))

(define (register-face! name doc #:inherit [inherit '()] #:fg [fg '()] #:bg [bg '()] #:weight [weight '()] #:slant [slant '()]
                        #:underline [underline '()] #:strike [strike '()] #:extend [extend '()] #:family [family '()] #:height [height '()])
  "Declare the face NAME, documented by DOC, with its own attributes.
FG and BG are colours, as the file's head says; WEIGHT is `light',
`normal' or `bold'; SLANT `normal' or `italic'; UNDERLINE #t, `line',
`curl', `dotted', `dashed' or `double', or one of those with a
colour: (curl red); STRIKE and EXTEND are booleans (an extending
background reaches the pane's edge where the face covers a line's
end); FAMILY is a font family, or `fixed' or `variable'; HEIGHT the
size, 1.3 or a percentage. FAMILY and HEIGHT are honoured only on
the `default' face, whole panes at a time: frontends are not asked
to change fonts or sizes inside a line, so set them there. An
attribute set to #f clears what the
face inherits of it. INHERIT is the face whose unset attributes this
one takes: not given, its longest declared dotted prefix; #f, none.
Defining it again in the same scope replaces the declaration, as a
reload does."
  (let ((spec (attributes-of fg bg weight slant underline strike extend family height)))
    (check-attributes "define-face" spec)
    (unless (or (null? inherit) (symbol? inherit) (eq? inherit #f)) (error "define-face: not a face to inherit:" inherit))
    ;; What of the chain is declared already must not lead back here.
    ;; In a package loading, its own faces are staged and not to be
    ;; seen yet; `face-table' still guards against what slips through.
    (let loop ((p (cond ((symbol? inherit) inherit) ((null? inherit) (face-prefix name)) (else #f))) (seen (list name)))
      (cond ((not p) #f)
            ((memq p seen) (error "define-face: it would inherit in a circle:" (reverse (cons p seen))))
            (else (loop (face-parent p) (cons p seen)))))
    (registry-add! %faces name (%make-face name doc inherit spec)))
  name)

(define-syntax define-face
  (syntax-rules ()
    "Declare the face NAME, documented by DOC.
The other arguments are keywords, as `register-face!' takes them."
    ((_ name doc arg ...) (register-face! 'name doc arg ...))))

;;; Settings

(define (set-face! name #:fg [fg '()] #:bg [bg '()] #:weight [weight '()] #:slant [slant '()]
                   #:underline [underline '()] #:strike [strike '()] #:extend [extend '()] #:family [family '()] #:height [height '()])
  "Set attributes of the face NAME.
FG, BG, WEIGHT, SLANT, UNDERLINE, STRIKE, EXTEND, FAMILY and HEIGHT
are as `define-face' takes them. A setting is owned by the scope
that makes it, as a theme is a package of them: where settings
stack, each attribute is taken from the newest setting that sets it,
over the face's declaration. An attribute set to #f clears what is
under it. The face need not be declared; the setting applies when it
is, and to its dotted specialisations until they set it themselves."
  (let ((attrs (attributes-of fg bg weight slant underline strike extend family height)))
    (check-attributes "set-face!" attrs)
    (registry-add! %face-settings name attrs))
  name)

(define (unset-face! name)
  "Take back this scope's setting of the face NAME.
A setting it shadowed is in effect again."
  (registry-remove! %face-settings name))

(define (load-theme name path)
  "Load the theme NAME from the file PATH.
A theme is a package: this is `load-package'. Loading another theme
does not unload this one; `unload-theme' does."
  (load-package name path))

(define (unload-theme name)
  "Unload the theme NAME: its face and palette settings go.
What they shadowed is in effect again."
  (unload-package name))

;;; The palette

(define (set-palette! name colour)
  "Give the palette name NAME the COLOUR, as it is written.
That is a palette name, \"#rrggbb\", or (colour ansi) with the ANSI
colour, 0 to 15, for frontends with only those. Owned by the scope
that sets it, so a theme's palette goes when the theme does."
  (unless (written-colour? colour) (error "set-palette!: not a colour:" colour))
  (registry-add! %palette name colour)
  name)

(define (palette-color name)
  "Return the colour of the palette name NAME as it is written, or #f."
  (registry-ref %palette name))

;;; Resolution

;; UPPER over LOWER, attribute by attribute. An entry of #f in UPPER is
;; kept: it clears what is under it when what is under it is merged in,
;; and what `face-attributes' returns shows it. Dropping it here would
;; clear nothing but the clear itself.
(define (merge-attributes lower upper)
  (fold (lambda (e acc) (cons e (remove (lambda (x) (eq? (car x) (car e))) acc)))
        lower upper))

;; The attributes a face itself says: its declaration, with its
;; settings over it, the oldest first.
(define (face-own name)
  (let ((d (find-face name)))
    (fold (lambda (attrs own) (merge-attributes own attrs))
          (if d (face-declared-spec d) '())
          (map car (reverse (registry-entries %face-settings name))))))

;; A name something answers to: declared, or set without a declaration
;; (a face still, over its prefix's attributes). Resolution takes such
;; a name for itself wherever it stands, prefixes included, as the
;; runtime's table has it.
(define (face-known? name) (or (find-face name) (pair? (registry-entries %face-settings name))))

;; The longest known name that is a dotted prefix of NAME, or #f.
(define (face-prefix name)
  (let loop ((parts (string-split (symbol->string name) ".")))
    (let* ((shorter (reverse (cdr (reverse parts)))) (candidate (and (pair? shorter) (string->symbol (string-join shorter ".")))))
      (cond ((null? shorter) #f)
            ((face-known? candidate) candidate)
            (else (loop shorter))))))

;; Who NAME inherits from, or #f.
(define (face-parent name)
  (let ((d (find-face name)))
    (cond ((eq? name 'default) #f)
          ((and d (symbol? (face-inherit d))) (face-inherit d))
          ((and d (eq? (face-inherit d) #f)) #f)
          (else (face-prefix name)))))

(define (face-attributes name)
  "Return the attributes of the face NAME, resolved through inheritance.
An alist (attribute . value) as the values are written (colours not
yet numbers); an attribute of #f is one cleared on the way, which for
#:underline, #:strike and #:extend also turns it off where the face
is drawn over another that has it. NAME is the face it stands for, as
a layer's name would be: its longest declared dotted prefix, if it
is not declared itself."
  ;; A name with settings but no declaration is a face still: its own
  ;; settings over its prefix's, so `set-face!' can target a dotted
  ;; name a package uses without declaring it.
  (let loop ((name (or (and (face-known? name) name) (face-prefix name))) (seen '()))
    (cond ((not name) '())
          ((memq name seen) (error "a face inherits in a circle:" (reverse (cons name seen))))
          (else (merge-attributes (loop (face-parent name) (cons name seen)) (face-own name))))))

;; A written colour as (r g b ansi), the palette followed, ansi #f or
;; 0 to 15; #f for no colour, or a palette name nothing gives.
(define (resolve-colour v)
  (let loop ((v v) (seen '()))
    (cond ((eq? v #f) #f)
          ((symbol? v) (and (not (memq v seen)) (let ((c (palette-color v))) (and c (loop c (cons v seen))))))
          ((string? v) (list (string->number (substring v 1 3) 16) (string->number (substring v 3 5) 16)
                             (string->number (substring v 5 7) 16) #f))
          ((pair? v) (let ((c (loop (car v) seen))) (and c (list (car c) (cadr c) (caddr c) (cadr v)))))
          (else #f))))

(define (explain-face name)
  "Return where each attribute of the face NAME comes from.
Each is (attribute cell value by): CELL is `setting', the face's own
name (its declaration) or the name of a face it inherits from, and BY
who made it (a package, `root' for the editor's own code and what is
evaluated in it), as `explain-option' says for options."
  (define (who scope) (or (scope-name scope) 'root))
  ;; As `face-attributes' resolves NAME, so a setting on an undeclared
  ;; name is not taken for the source of what its prefix gives.
  (define face (or (and (face-known? name) name) (face-prefix name)))
  ;; Where ATTR is set for NAME, its own cell first, then its parents.
  (define (source name attr seen)
    (cond ((or (not name) (memq name seen)) #f)
          ((find (lambda (e) (assq attr (car e))) (registry-entries %face-settings name))
           => (lambda (e) (list 'setting (who (cdr e)))))
          ((let ((d (find-face name))) (and d (assq attr (face-declared-spec d))))
           (list name (who (registry-owner %faces name))))
          (else (source (face-parent name) attr (cons name seen)))))
  (map (lambda (e) (let ((from (source face (car e) '()))) (list (car e) (and from (car from)) (cdr e) (and from (cadr from)))))
       (face-attributes face)))

(define (face-table)
  "Return the declared faces for the runtime, one list per face.
Each is (name fg bg weight slant underline strike extend family
height), resolved through inheritance and the palette: a colour (r g
b ansi), ansi #f or 0 to 15, an underline (style colour) with style
`none' for off in particular, a switch #t, `none' or #f, a height a
percentage; an attribute not set is #f."
  (define (attr attrs a) (let ((e (assq a attrs))) (and e (cdr e))))
  ;; Straight from the alist, #f of its own included: for the switches,
  ;; an attribute given as #f is off, where no entry at all is unset.
  (define (switch attrs a)
    (let ((e (assq a attrs))) (cond ((not e) #f) ((cdr e) #t) (else 'none))))
  (define (entry name)
    ;; A face that does not resolve (it inherits in a circle a package's
    ;; staged definitions could not be checked against) is given plain,
    ;; so one bad face does not freeze the table for the others.
    (let* ((attrs (guard (e (#t '())) (face-attributes name)))
           (u (assq 'underline attrs))
           (family (attr attrs 'family))
           (height (attr attrs 'height)))
      (list name
            (resolve-colour (attr attrs 'fg))
            (resolve-colour (attr attrs 'bg))
            (attr attrs 'weight)
            (attr attrs 'slant)
            (cond ((not u) #f)
                  ((not (cdr u)) '(none #f))
                  ((pair? (cdr u)) (list (car (cdr u)) (resolve-colour (cadr (cdr u)))))
                  (else (list (if (eq? (cdr u) #t) 'line (cdr u)) #f)))
            (switch attrs 'strike)
            (switch attrs 'extend)
            (cond ((symbol? family) (symbol->string family)) (else family))
            (and height (max 1 (inexact->exact (round (if (< height 10) (* height 100) height))))))))
  ;; The declared faces, and any name with settings but no declaration:
  ;; set-face! on a dotted name a package uses must reach the table.
  (map entry (append (face-names) (remove find-face (registry-keys %face-settings)))))

;;; The palette (Catppuccin Mocha) and the faces built in.
;;;
;;; A theme is a package of set-palette! and set-face! calls over
;;; these; the second element of a colour that has one is its ANSI
;;; number for a 16-colour frontend.

(for-each (lambda (e) (set-palette! (car e) (cdr e)))
          '((base . ("#1e1e2e" 0)) (mantle . "#181825") (crust . ("#11111b" 0))
            (text . ("#cdd6f4" 7)) (subtext . ("#a6adc8" 7)) (overlay . ("#7f849c" 8)) (overlay0 . ("#6c7086" 8)) (overlay2 . "#9399b2")
            (surface . ("#45475a" 8)) (surface0 . ("#313244" 8))
            (mauve . ("#cba6f7" 5)) (red . ("#f38ba8" 1)) (peach . ("#fab387" 3)) (yellow . ("#f9e2af" 3))
            (green . ("#a6e3a1" 2)) (teal . ("#94e2d5" 6)) (sky . ("#89dceb" 6)) (blue . ("#89b4fa" 4)) (rosewater . "#f5e0dc")
            ;; The sixteen ANSI colours, in this palette's terms: what a
            ;; terminal buffer's own SGR colours are drawn with, each
            ;; itself at sixteen colours.
            (ansi-0 . ("#45475a" 0)) (ansi-1 . ("#f38ba8" 1)) (ansi-2 . ("#a6e3a1" 2)) (ansi-3 . ("#f9e2af" 3))
            (ansi-4 . ("#89b4fa" 4)) (ansi-5 . ("#f5c2e7" 5)) (ansi-6 . ("#94e2d5" 6)) (ansi-7 . ("#bac2de" 7))
            (ansi-8 . ("#585b70" 8)) (ansi-9 . ("#f38ba8" 9)) (ansi-10 . ("#a6e3a1" 10)) (ansi-11 . ("#f9e2af" 11))
            (ansi-12 . ("#89b4fa" 12)) (ansi-13 . ("#f5c2e7" 13)) (ansi-14 . ("#94e2d5" 14)) (ansi-15 . ("#a6adc8" 15))))

(define-face default "The text's own face: what nothing else sets, it colours." #:fg 'text #:bg 'base #:inherit #f)
(define-face shadow "Text of lesser importance." #:fg 'overlay)
;;; The yellow of highlight and the cyan of lazy-highlight are bright
;;; at sixteen colours, dark enough in truecolour: their foreground
;;; writes both, so a sixteen-colour terminal keeps the black text it
;;; had when these colours were its own (30;43, 30;46).
(define-face highlight "What is pointed out, for now." #:fg '("#cdd6f4" 0) #:bg '("#5b4f2c" 3))
(define-face region "The selected text." #:bg 'surface)
(define-face cursor "The caret of the pane that has the keys." #:bg 'rosewater)
(define-face cursor.inactive "The caret of a pane without the keys." #:bg 'overlay)
(define-face mode-line "The mode line of the focused pane." #:fg 'text #:bg 'surface #:weight 'bold)
(define-face mode-line.inactive "The mode line of another pane." #:fg 'overlay2 #:bg '("#262638" 0))
(define-face border "The divider between panes." #:fg 'surface0 #:bg 'surface0)
(define-face line-number "The gutter's numbers." #:fg 'overlay0)
(define-face line-number.current "The number of the caret's line." #:fg 'text)
(define-face completion "In-buffer completion's popup." #:fg 'text #:bg 'surface0)
(define-face completion.selected "The popup's selected candidate." #:bg '("#00415e" 4))
(define-face minibuffer.selected "The minibuffer's selected candidate." #:fg 'text #:bg 'surface)
(define-face prompt "A prompt: the minibuffer's, a REPL's." #:fg 'mauve #:weight 'bold)
(define-face key "A key, shown beside what it does." #:fg 'teal)
(define-face link "Something RET follows." #:fg 'blue #:underline 'line)
(define-face heading "A heading in a listing." #:weight 'bold)

(define-face comment "Comments." #:fg 'overlay #:slant 'italic)
(define-face keyword "Language keywords." #:fg 'mauve #:weight 'bold)
(define-face string "Strings." #:fg 'green)
(define-face function "Function names, defined and called." #:fg 'blue)
(define-face type "Type names." #:fg 'yellow)
(define-face variable "Variable names.")
(define-face constant "Constants." #:fg 'peach)
(define-face number "Numbers." #:fg 'peach)
(define-face operator "Operators." #:fg 'sky)
(define-face punctuation "Punctuation." #:fg 'subtext)
(define-face tag "Tags, and what names a thing in markup." #:fg 'mauve)
(define-face attribute "Attributes in markup." #:fg 'teal)
;; The rest of the tree-sitter captures' common names, so the queries'
;; dotted specialisations land somewhere meant for them.
(define-face boolean "True and false." #:inherit 'constant)
(define-face constructor "Constructors and what is made like one." #:inherit 'type)
(define-face module "Module and namespace names." #:inherit 'type)
(define-face property "Keys and fields: what a value is held under." #:fg 'blue)
(define-face label "Labels." #:fg 'peach)
(define-face delimiter "Delimiters, in languages whose queries name them so." #:inherit 'punctuation)
(define-face escape "Escapes in strings." #:fg 'peach)
(define-face markup "Markup text.")
(define-face markup.heading "A heading in markup." #:inherit 'heading #:fg 'blue)
(define-face markup.strong "Strong text in markup." #:weight 'bold)
(define-face markup.emphasis "Emphasised text in markup." #:slant 'italic)
(define-face markup.raw "Code and literal text in markup." #:inherit 'string)
(define-face markup.link "A link in markup." #:inherit 'link)

(define-face match "What matched, among the minibuffer's candidates." #:fg 'sky #:weight 'bold)
(define-face isearch "The match a search is at." #:fg '("#ffffff" 15) #:bg '("#8b3a8b" 5))
(define-face lazy-highlight "A search's other matches." #:fg '("#cdd6f4" 0) #:bg '("#2f5a6b" 6))
(define-face success "What went well." #:fg 'green #:weight 'bold)
(define-face info "What is worth knowing." #:fg 'sky)
(define-face warning "What is wrong, but not fatal." #:fg 'yellow #:weight 'bold)
(define-face warning.stale "An excerpt whose source changed since it was shown.")
(define-face error "What failed." #:fg 'red #:weight 'bold)
