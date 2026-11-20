;;; Package directories: manifests read as data, the libraries a package
;;; provides and those of the packages it depends on, without the editor.
(import (techne) (techne package) (techne path) (techne uuid))
(require "../test.scm")

(define root (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-package-" (uuid-v4)))
(define (at . parts) (apply path-join root parts))
(define (write-file! file . forms)
  (make-directory (path-directory file))
  (call-with-output-file file (lambda (port) (for-each (lambda (f) (write f port) (newline port)) forms))))
(define (package! name . clauses)
  (write-file! (at (symbol->string name) "techne-package.scm") `(package ,name ,@clauses)))
(define (library! package name . body)
  (write-file! (at (symbol->string package) "lib" (symbol->string package) (string-append (symbol->string name) ".sld"))
               `(define-library (,package ,name) ,@body)))
(define (why thunk) (guard (e (#t (condition/report-string e))) (thunk) #f))
(define (failed? thunk) (and (why thunk) #t))

;; A manifest is data: its clauses, and the form it was first written in.
(write-file! (at "editing" "entry.scm") '(define loaded #t))
(package! 'editing
          '(depends (alpha (path "../alpha")))
          '(editor "entry.scm" (commands (edit "Edit it.")) (options (edit-count 2 integer "How often."))))
(let ((m (read-package-manifest (at "editing"))))
  (check "name" 'editing (manifest-name m))
  (check "directory" (file-truename (at "editing")) (manifest-directory m))
  (check "dependencies, by directory" (list (cons 'alpha (path-join (file-truename (at "editing")) "../alpha")))
         (manifest-dependencies m))
  (check "editor entry, commands and options"
         (list (file-truename (at "editing" "entry.scm")) '((edit "Edit it.")) '((edit-count 2 integer "How often.")))
         (manifest-editor m)))
(write-file! (at "first" "entry.scm") '(define loaded #t))
(write-file! (at "first" "techne-package.scm") '(package first "entry.scm" ((go "Go.")) ((speed 3 integer "How fast."))))
(let ((m (read-package-manifest (at "first"))))
  (check "the first form is an editor clause"
         (list (file-truename (at "first" "entry.scm")) '((go "Go.")) '((speed 3 integer "How fast.")))
         (manifest-editor m))
  (check "and depends on nothing" '() (manifest-dependencies m)))
(package! 'plain)
(check "a package of libraries only" #f (manifest-editor (read-package-manifest (at "plain"))))
(for-each
 (lambda (case)
   (apply package! 'broken (cdr case))
   (check (car case) #t (failed? (lambda () (read-package-manifest (at "broken"))))))
 '(("an unknown clause" (version "1"))
   ("a clause twice" (depends) (depends))
   ("an entry outside the package" (editor "../editing/entry.scm" (commands (x "X."))))
   ("an entry that is missing" (editor "absent.scm" (commands (x "X."))))
   ("no commands" (editor "entry.scm" (commands)))
   ("a default its schema refuses" (editor "entry.scm" (commands (x "X.")) (options (n "two" integer "N."))))
   ("an absolute dependency" (depends (alpha (path "/alpha"))))
   ("itself as a dependency" (depends (broken (path "../broken"))))
   ("a dependency twice" (depends (alpha (path "../alpha")) (alpha (path "../other"))))))
(check "no manifest" #t (failed? (lambda () (read-package-manifest root))))

;; Using a package finds its libraries and those of what it depends on,
;; under their `lib` directories, and loads nothing.
(package! 'alpha)
(library! 'alpha 'greet '(export greet loads) '(import (scheme base))
          '(begin (define loads (list 'loaded)) (define (greet who) (string-append "hello, " who))))
(package! 'beta '(depends (alpha (path "../alpha"))))
(library! 'beta 'twice '(export twice) '(import (scheme base) (alpha greet))
          '(begin (define (twice who) (string-append (greet who) " and " (greet who)))))
(package! 'gamma '(depends (beta (path "../beta"))))
(check "before, nothing is found" #t (failed? (lambda () (eval '(import (beta twice))))))
(check "use returns the manifest" 'gamma (manifest-name (use-package-directory! (at "gamma"))))
(check "the same one again" #t (eq? (use-package-directory! (at "gamma")) (use-package-directory! (at "gamma"))))
(check "its dependency's libraries" (file-truename (at "beta" "lib")) (%library-provider "beta"))
(check "and that one's" (file-truename (at "alpha" "lib")) (%library-provider "alpha"))
(check "a package without libraries owns its name too" (path-join (file-truename (at "gamma")) "lib") (%library-provider "gamma"))
(define (loaded? name) (and (member name (loaded-modules)) #t))
(check "nothing was loaded" #f (loaded? "(alpha greet)"))
(import (beta twice) (alpha greet))
(check "an import loads it" #t (loaded? "(alpha greet)"))
(check "a library importing its dependency's" "hello, you and hello, you" (twice "you"))
;; One instance, whoever imports it.
(check "shared" #t (eq? loads (eval 'loads (environment '(alpha greet)))))

;; What a package provides is the world's, not a generation's: the package
;; reloaded or unloaded, the others keep the same library, and so does it.
(package! 'delta)
(library! 'delta 'state '(export token) '(import (scheme base)) '(begin (define token (list 'instance))))
(write-file! (at "delta" "entry.scm") '(import (delta state)) '(define (mine) token))
(use-package-directory! (at "delta"))
(define (delta-token) ((eval 'mine (package-module (find-package 'delta)))))
(load-package 'delta (at "delta" "entry.scm"))
(define first-token (delta-token))
(check "another importer shares it" #t (eq? first-token (eval 'token (environment '(delta state)))))
(load-package 'delta (at "delta" "entry.scm"))
(check "the next generation keeps it" #t (eq? first-token (delta-token)))
(unload-package 'delta)
(check "unloaded, it is there still" #t (eq? first-token (eval 'token (environment '(delta state)))))

;; A name belongs to one directory: its libraries are found there only.
(write-file! (at "elsewhere" "alpha" "other.sld") '(define-library (alpha other) (export x) (import (scheme base)) (begin (define x 1))))
(write-file! (at "elsewhere" "main.scm") '(import (alpha other)))
(check "not beside the importer" "import: no library (alpha other)"
       (why (lambda () (load-package 'elsewhere (at "elsewhere" "main.scm")))))
(write-file! (at "copy" "alpha" "techne-package.scm") '(package alpha))
(check "not in another directory" #t (failed? (lambda () (use-package-directory! (at "copy" "alpha")))))
(package! 'techne)
(check "not Techne's own" #t (failed? (lambda () (use-package-directory! (at "techne")))))

;; A set of packages that cannot be used leaves nothing of itself.
(package! 'lost '(depends (nowhere (path "../nowhere"))))
(check "a dependency that is missing" #t (failed? (lambda () (use-package-directory! (at "lost")))))
(package! 'misnamed '(depends (delta (path "../plain"))))
(check "a dependency of another name" #t (failed? (lambda () (use-package-directory! (at "misnamed")))))
(package! 'ring-a '(depends (ring-b (path "../ring-b"))))
(package! 'ring-b '(depends (ring-a (path "../ring-a"))))
(library! 'ring-a 'x '(export x) '(import (scheme base)) '(begin (define x 1)))
(check "a circle" #t (failed? (lambda () (use-package-directory! (at "ring-a")))))
(check "provides nothing" #f (%library-provider "ring-a"))
(package! 'ring-b)
(check "mended, it is used" 'ring-a (manifest-name (use-package-directory! (at "ring-a"))))
(check "and provides" (file-truename (at "ring-a" "lib")) (%library-provider "ring-a"))

;; Packages of one name in two directories, one depending on the other
;; through a third: the first is not made known yet when the second is met.
(package! 'twin '(depends (between (path "../between"))))
(package! 'between '(depends (twin (path "../far/twin"))))
(write-file! (at "far" "twin" "techne-package.scm") '(package twin))
(library! 'between 'x '(export x) '(import (scheme base)) '(begin (define x 1)))
(check "a name met again below itself" #t
       (and (string-contains (or (why (lambda () (use-package-directory! (at "twin")))) "") "Another directory holds") #t))
(check "leaves nothing" #f (%library-provider "between"))

;; What cannot be provided is found before anything changes: libraries
;; provided from elsewhere, loaded from elsewhere already, or a `lib` that
;; is no directory, each in the package another one depends on.
(define (left-whole? name dependency reason)
  (package! name `(depends (,dependency (path ,(string-append "../" (symbol->string dependency))))))
  (library! name 'x '(export x) '(import (scheme base)) '(begin (define x 1)))
  (list (and (string-contains (or (why (lambda () (use-package-directory! (at (symbol->string name))))) "") reason) #t)
        (%library-provider (symbol->string name))))
(make-directory (at "taken"))
(%provide-libraries! "omega" (at "taken"))
(package! 'omega)
(library! 'omega 'x '(export x) '(import (scheme base)) '(begin (define x 1)))
(check "libraries provided from elsewhere" '(#t #f) (left-whole? 'zeta 'omega "are provided from"))
(write-file! (at "loose" "early" "x.sld") '(define-library (early x) (export x) (import (scheme base)) (begin (define x 1))))
(write-file! (at "loose" "main.scm") '(import (early x)))
(load-package 'loose (at "loose" "main.scm"))
(package! 'early)
(library! 'early 'x '(export x) '(import (scheme base)) '(begin (define x 2)))
(check "a library loaded from elsewhere" '(#t #f) (left-whole? 'eta 'early "library (early x) is loaded from elsewhere"))
(package! 'flat)
(write-file! (at "flat" "lib") 'not-a-directory)
(check "a lib that is no directory" '(#t #f) (left-whole? 'theta 'flat "is not a directory"))

;; A package's files import, of what packages provide, its own libraries
;; and those of the packages it depends on itself: not those further on,
;; loaded already or not, which are not run for the attempt either.
(define (refused? thunk) (and (string-contains (or (why thunk) "") "does not depend on alpha") #t))
(library! 'alpha 'fresh '(export fresh) '(import (scheme base)) '(begin (define fresh 1)))
(library! 'gamma 'own '(export own) '(import (scheme base) (beta twice)) '(begin (define own (twice "me"))))
(write-file! (at "gamma" "direct.scm") '(import (beta twice) (gamma own)) '(define direct (list own (twice "a"))))
(write-file! (at "gamma" "beyond.scm") '(import (alpha greet)))
(write-file! (at "gamma" "unloaded.scm") '(import (only (alpha fresh) fresh)))
(check "what it depends on, and its own" 1 (load-package 'gamma-direct (at "gamma" "direct.scm")))
(check "not what that one depends on" #t (refused? (lambda () (load-package 'gamma-beyond (at "gamma" "beyond.scm")))))
(check "nor one not loaded yet" #t (refused? (lambda () (load-package 'gamma-unloaded (at "gamma" "unloaded.scm")))))
(check "which stays unloaded" #f (loaded? "(alpha fresh)"))
(check "a file in no package imports any" 1 (eval 'fresh (environment '(alpha fresh))))
(unload-package 'gamma-direct)

;; `package-closure` makes the checks of `use-package-directory!` without
;; making anything known; neither goes outside the directory it is given.
(package! 'inside '(depends (alpha (path "../alpha"))))
(check "the packages to make known" '(inside) (map manifest-name (package-closure (at "inside"))))
(check "not outside a repository" #t (failed? (lambda () (package-closure (at "inside") #:within (at "inside")))))
(check "nor used from there" #t (failed? (lambda () (use-package-directory! (at "inside") #:within (at "inside")))))
(check "within it" 'inside (manifest-name (use-package-directory! (at "inside") #:within root)))
(check "known, there is nothing to make known" '() (package-closure (at "inside")))
;; One known already, and what it depends on, are held to it too.
(check "known, and outside" #t (failed? (lambda () (use-package-directory! (at "inside") #:within (at "inside")))))
(make-directory (at "repo"))
(write-file! (at "repo" "upper" "techne-package.scm") '(package upper (depends (inside (path "../../inside")))))
(write-file! (at "repo" "top" "techne-package.scm") '(package top (depends (upper (path "../upper")))))
(use-package-directory! (at "repo" "upper"))
(check "through a package known already" #t
       (and (string-contains (or (why (lambda () (use-package-directory! (at "repo" "top") #:within (at "repo")))) "")
                             "outside its repository")
            #t))
(check "which is not made known" #f (%library-provider "top"))

;; The packages of a repository, side by side or deeper, by directory.
(check "a package's directory is its own" '("") (package-directories (at "alpha")))
(let ((found (package-directories root)))
  (check "side by side" #t (and (member "alpha" found) (member "beta" found) #t))
  (check "below a directory that is none" #t (and (member "copy/alpha" found) #t))
  (check "not a package's own files" #f (and (member "alpha/lib" found) #t)))

(delete-path root #t)
(test-summary)
