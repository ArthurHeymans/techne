;;; The packages Techne ships, in packages/ beside its Lisp, and a package
;;; of this repository that depends on one of them without the editor.
(import (techne) (techne package) (techne path))
(require "../test.scm")

;; Each manifest is one, named as its directory, and depends on nothing:
;; the editor relies on that to advertise them at start without
;; `(techne package)` (lisp/editor/builtins.scm).
(define (here) #t)
(define tests (path-directory (car (procedure-location here))))
(define shipped (path-join tests "../../packages"))
(define shipped-names
  (map (lambda (entry) (substring entry 0 (- (string-length entry) 1)))
       (filter (lambda (entry) (string-suffix? "/" entry)) (directory-list shipped))))
(check "packages are shipped" #t (pair? shipped-names))
(for-each (lambda (name)
            (let ((m (read-package-manifest (path-join shipped name))))
              (check (string-append name ": named as its directory") name (symbol->string (manifest-name m)))
              (check (string-append name ": depends on nothing") '() (manifest-dependencies m))))
          shipped-names)

;; A package without the editor depending on mail, which has it too: mail's
;; libraries are found through the manifests, and nothing of the editor is
;; loaded.
(use-package-directory! (path-join tests "fixtures/digest"))
(import (digest subjects))
(check "mail's MIME library, from a package depending on mail" '("Hello" "✓ café")
       (subjects '("Subject: Hello\r\n\r\nbody" "Subject: =?UTF-8?B?4pyTIGNhZsOp?=\r\n\r\nbody")))
(check "without the editor" #f (and (member "(techne editor)" (loaded-modules)) #t))
(test-summary)
