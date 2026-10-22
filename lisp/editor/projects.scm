;;; Projects: the directory a file belongs to, its version-control root;
;;; and searching the project's files as you type (consult-ripgrep is the
;;; reference). The search runs in a service (services/search.scm), a
;;; world of its own: listing and reading the files holds up neither
;;; keys nor frames, and typing cancels the search for the input before.

(require "session.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "files.scm")
(require "minibuffer.scm")
(require "buffers.scm")

(provide project-root search-project)

(define (project-root dir)
  "Return the root of the project the directory DIR is in.
That is the nearest directory up from DIR with a .jj or .git in it,
else DIR itself; with a slash, as DIR has."
  (let up ((d dir))
    (cond ((or (file-exists? (string-append d ".jj")) (file-exists? (string-append d ".git"))) d)
          ((string=? d "/") dir)
          (else (up (directory-of (substring d 0 (- (string-length d) 1))))))))

;; This file, which the service's is found beside.
(define %here (current-module))

;; The search service, made when first asked for.
(define %search #f)

(define (search-service)
  (or %search
      (begin
        (set! %search (make-service "search" (string-append (directory-of %here) "services/search.scm") #:files #t))
        %search)))

;; The candidates searching shows at most.
(define search-limit 1000)

;; A candidate of the match M the service found, (path name line
;; position text id revision): in one of DOCUMENTS, a location as of the
;; revision searched; else a line of a file.
(define (match-candidate documents m)
  (let* ((path (list-ref m 0)) (line (list-ref m 2)) (id (list-ref m 5))
         (d (and id (find (lambda (d) (= (document-id d) id)) documents))))
    (candidate (list-ref m 4)
               #:annotation (string-append (list-ref m 1) ":" (number->string line))
               #:target (target 'location (if d (location d (list-ref m 3) #:revision (list-ref m 6)) (file-location path line))))))

(define-command (search-project s n)
  "Go to a line of a file of this project, previewing each while choosing.
The project is the version-control root above the focused file (see
`project-root`). The lines with every word of the input are found as
you type, from three characters on; open files are searched as they
are, unsaved edits and all."
  (let* ((root (project-root (default-directory s)))
         (documents (filter document-path (map buffer-document (buffer-list))))
         ;; The files are listed again for each search, not each key.
         (fresh #t))
    (completing-read s (string-append "Search " root ": ")
                     (async-source
                      (lambda (input)
                        (let ((found (service-call (search-service) "search"
                                                   (list root (pattern-parts input) search-limit (map document-path documents) fresh)
                                                   documents)))
                          (set! fresh #f)
                          (map (lambda (m) (match-candidate documents m)) found))))
                     #:preview (lambda (s c) (act-default! s (candidate-target c))))))
