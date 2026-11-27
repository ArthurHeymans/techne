;;; Headless editor tests for single-key target actions (Embark-style) in find-file and at point.
(import (techne test))
(test-file #:environment 'editor)
(require "../main.scm")
(require "../session.scm")
(require "../dispatch.scm")
(require "../minibuffer.scm")
(require "../which-key.scm")
(require "../targets.scm")
(require "../buffers.scm")
(require "../api.scm")
(require "../packages/dired.scm")

(define-test "single-key file & directory actions in find-file"
  (call-with-temporary-directory
   (lambda (tmp)
     (let* ((f1 (string-append tmp "/alpha.txt"))
            (f2 (string-append tmp "/beta.txt"))
            (sub (string-append tmp "/sub/")))
       (replace-file f1 "hello from alpha\n")
       (replace-file f2 "hello from beta\n")
       (make-directory sub)
       (let* ((d (file-document f1))
              (s (start-session (make-view d "user") "emacs")))
         ;; 1. Open find-file: initial input is directory tmp/ -> C-c a acts on directory target!
         (press-keys s "C-x C-f")
         (press-keys s "C-c a")
         (let ((hints (editor-key-hints s)))
           (check-true "action prompt shows immediate key hints" (pair? hints))
           (check-true "directory hints include D (delete) and + (mkdir)"
                       (and (assoc "D" hints) (assoc "+" hints) #t)))
         ;; Press + to create a subdirectory
         (press-keys s "+")
         (type-text s "created")
         (press-keys s "RET")
         (check-true "+ created subdirectory" (path-exists? (string-append tmp "/created")))

         ;; 2. Act on file candidate beta.txt with c (copy) -> copy.txt
         (press-keys s "C-x C-f")
         (type-text s "beta.txt")
         (press-keys s "C-.")
         (let ((hints (editor-key-hints s)))
           (check-true "file hints include d (delete), r (rename), c (copy), w (copy path)"
                       (and (assoc "d" hints) (assoc "r" hints) (assoc "c" hints) (assoc "w" hints) #t)))
         (press-keys s "c")
         (type-text s "copied.txt")
         (press-keys s "RET")
         (check-true "c copied beta.txt to copied.txt" (path-exists? (string-append tmp "/copied.txt")))

         ;; 3. Rename copied.txt -> moved.txt via r
         (press-keys s "C-x C-f")
         (type-text s "copied.txt")
         (press-keys s "C-;")
         (press-keys s "r")
         (type-text s "moved.txt")
         (press-keys s "RET")
         (check "r removed old name" #f (path-exists? (string-append tmp "/copied.txt")))
         (check-true "r created new name" (path-exists? (string-append tmp "/moved.txt")))

         ;; 4. Delete moved.txt via d + yes confirmation
         (press-keys s "C-x C-f")
         (type-text s "moved.txt")
         (press-keys s "C-c a")
         (press-keys s "d")
         (type-text s "yes")
         (press-keys s "RET")
         (check "d deleted moved.txt" #f (path-exists? (string-append tmp "/moved.txt")))

         ;; 5. ? falls back to fuzzy completing-read over actions, and C-g cancels cleanly
         (press-keys s "C-x C-f")
         (type-text s "beta.txt")
         (press-keys s "C-;")
         (press-keys s "?")
         (check-true "? opened minibuffer search for actions" (minibuffer-open? s))
         (press-keys s "C-g")
         (check "C-g closed action search" #f (minibuffer-open? s))

         ;; 6. S collects minibuffer candidates into *Targets* buffer
         (press-keys s "C-x C-f")
         (press-keys s "C-c S")
         (check-true "C-c S opened *Targets* buffer"
                     (string-prefix? "*Targets:" (buffer-name (current-buffer s))))
         (press-keys s "q")

         ;; 7. Symlink via s on beta.txt
         (press-keys s "C-x C-f")
         (type-text s "beta.txt")
         (press-keys s "C-;")
         (press-keys s "s")
         (type-text s "beta-link.txt")
         (press-keys s "RET")
         (check-true "s created symlink beta-link.txt" (path-exists? (string-append tmp "/beta-link.txt")))

         ;; 8. Deleting a directory symlink with trailing slash never deletes referent contents
         (let* ((real-dir (string-append tmp "/real-dir"))
                (inside (string-append real-dir "/keep.txt"))
                (dir-link (string-append tmp "/dir-link")))
           (make-directory real-dir)
           (replace-file inside "precious\n")
           (call-with-process "ln" (list "-s" real-dir dir-link) (lambda (p) (process-wait p)))
           (delete-path-and-buffers! s (string-append dir-link "/") #:recursive #t)
           (check "symlink itself removed" #f (file-attributes dir-link))
           (check-true "referent file preserved" (path-exists? inside)))

         ;; 9. Directory default action is visit-directory (never delete-directory-target)
         (let ((dir-acts (actions-for 'directory)))
           (check "directory default is visit-directory" 'visit-directory (action-name (car dir-acts))))

         ;; 10. Fallback target-at URL & region actions (u upcase)
         (let* ((ud (make-document "Visit https://example.com today\n"))
                (us (start-session (make-view ud "user") "emacs")))
           (let ((t (target-at ud 8)))
             (check "target-at detects URL" 'url (and t (target-kind t)))
             (check "target-at URL value" "https://example.com" (and t (target-value t))))
           ;; Select "Visit" (0..5) and act-at-point with 'u' (upcase)
           (view-set-ranges! (session-view us) '((0 5)) 0)
           (run-command us 'act-at-point 1)
           (press-keys us "u")
           (check "region action u upcased selection" "VISIT https://example.com today\n" (document-string ud))))))))
