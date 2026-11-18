;;; Repository state independent of the status buffers and VC package generation.

(import (techne crypto))
(require "../../files.scm")


(provide vc-recovery-state vc-recovery-begin! vc-recovery-complete! vc-recovery-acknowledge!)

(define %recovery (make-hash-table))

(define (recovery-path repo)
  (state-file (string-append "vc-write-" (hex-encode (sha-256 (string-append (symbol->string (car repo)) "\n" (cadr repo)))))))

(define (read-evidence path fallback)
  (guard (e (#t fallback))
    (let ((value (call-with-input-file path read)))
      (if (and (list? value) (memv (length value) '(3 6 7)) (every string? value)) value fallback))))

(define (write-evidence-file path evidence)
  (call-with-output-file path (lambda (port) (write evidence port))))

(define (vc-recovery-state repo)
  "Return the table of write recovery evidence for REPO."
  (or (hash-table-ref/default %recovery repo #f)
      (let ((state (make-hash-table)))
        (let* ((path (recovery-path repo)) (pending (string-append path ".pending")))
          (when (file-exists? pending)
            (hash-table-set! state 'unresolved (read-evidence (string-append pending "/attempt")
                                                             '("Interrupted write" "Details unavailable" "unknown"))))
          (when (file-exists? (string-append path ".last"))
            (hash-table-set! state 'last-unresolved (read-evidence (string-append path ".last") #f))))
        (hash-table-set! %recovery repo state)
        state)))

(define (vc-recovery-begin! repo title before #:destination [destination '()])
  "Record the remote attempt TITLE for REPO before dispatch.
BEFORE is evidence read before trying the write. DESTINATION, when set,
is a list of the source commit, remote name, ref and endpoint digest."
  (let* ((st (vc-recovery-state repo)) (pending (string-append (recovery-path repo) ".pending"))
         (attempt (append (list title before (hex-encode (random-bytes 16))) destination)))
    ;; Directory creation is exclusive. An incomplete attempt file isn't
    ;; evidence that the dispatch can be replayed.
    (create-directory pending #o700)
    (hash-table-set! st 'unresolved attempt)
    (write-evidence-file (string-append pending "/attempt") attempt)
    attempt))

(define (vc-recovery-complete! repo)
  "Release the pending record for REPO after receiving an exit status."
  (delete-path (string-append (recovery-path repo) ".pending") #t)
  (hash-table-set! (vc-recovery-state repo) 'unresolved #f))

(define (vc-recovery-acknowledge! repo attempt observed)
  "Keep ATTEMPT and OBSERVED as evidence for REPO.
Permit new writes without asserting what the previous command did."
  (let* ((path (recovery-path repo)) (temp (string-append path ".pending/observed"))
         (evidence (list (car attempt) (cadr attempt) observed)))
    (unless (equal? attempt (read-evidence (string-append path ".pending/attempt")
                                          '("Interrupted write" "Details unavailable" "unknown")))
      (error "The pending write changed; inspect it again"))
    (when (file-exists? temp) (delete-file temp))
    (write-evidence-file temp evidence)
    (rename-file temp (string-append path ".last"))
    (vc-recovery-complete! repo)
    (hash-table-set! (vc-recovery-state repo) 'last-unresolved evidence)))
