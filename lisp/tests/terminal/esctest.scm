;;; Run Ebb's focused VT compatibility suite through the actual Techne
;;; terminal frontend, its Scheme emulator, and the child's real PTY.
;;; Usage: techne run lisp/tests/terminal/esctest.scm ESCTEST2 TECHNE
;;; ESCTEST2 revision: 664be3cf2c1e3f06bc93a8bafb48a0db83c607db.
(import (techne) (techne uuid))

(define args (cdr (command-line)))
(unless (= (length args) 2) (error "Expected the esctest2 directory and Techne executable"))
(define suite (string-append (file-truename (car args)) "/esctest"))
(define binary (file-truename (cadr args)))
(define dir (string-append (or (get-environment-variable "TMPDIR") "/tmp") "/techne-vt-" (uuid-v4)))
(define logfile (string-append dir "/esctest.log"))
(define notes (string-append dir "/notes"))
(define selection
  (string-append
   "(APCTests.test_APC_Basic|BSTests|CHATests|CHTTests|CNLTests|CPLTests|"
   "CRTests|CUBTests|CUDTests|CUFTests|CUPTests|CUUTests|DCHTests|DLTests|"
   "DECALNTests|DECCRATests|DECERATests|DECFRATests|DECRQMTests|"
   "DECRQSSTests|DECSEDTests|DECSELTests|DECSERATests|"
   "ECHTests|EDTests|ELTests|"
   "ICHTests|ILTests|INDTests|LFTests|NELTests|RITests|"
   "DECSTRTests.test_DECSTR_(CursorStaysPut|DECSC|IRM|STBM)|"
   "RISTests.test_RIS_(ClearsScreen|CursorToOrigin|ResetTabs|ResetDECCOLM)|"
   "XtermWinopsTests.test_XtermWinops_ResizeChars_BothParameters)"))
(define expected "*** 258 tests passed, 28 known bugs, 0 tests failed ***")

(define (tmux . args)
  (call-with-process "tmux" (append (list "-S" (string-append dir "/tmux") "-f" "/dev/null") args)
    (lambda (p)
      (let ((err (spawn (lambda () (process-read-all p 'stderr))))
            (out (process-read-all p 'stdout)))
        (let ((status (process-wait p)))
          (unless (eqv? status 0) (error "tmux failed" args (await err)))
          out)))
    #:environment `(("XDG_STATE_HOME" . ,dir) ("XDG_CONFIG_HOME" . ,dir))))
(define (keys . args) (apply tmux "send-keys" args))
(define (frame) (tmux "capture-pane" "-p"))
(define (await-frame fragment seconds)
  (let wait ((left (* seconds 10)))
    (let ((text (frame)))
      (unless (string-contains text fragment)
        (when (= left 0) (error "Timed out waiting for terminal frame" fragment text))
        (sleep 100)
        (wait (- left 1))))))
(define (quote-sh s) (string-append "'" (string-join (string-split s "'") "'\\''") "'"))

(unless (file-exists? (string-append suite "/esctest.py")) (error "Cannot find esctest.py" suite))
(create-directory dir #o700)
(call-with-output-file notes (lambda (p) (display "Techne VT compatibility\n" p)))
(dynamic-wind
 (lambda () #f)
 (lambda ()
   (tmux "new-session" "-d" "-x" "80" "-y" "27" binary "term" notes)
   (await-frame "Techne VT compatibility" 15)
   (keys "M-x")
   (await-frame "M-x" 10)
   (keys "-l" "terminal-command")
   (keys "Enter")
   (await-frame "Terminal command:" 10)
   (keys "-l" (string-append "cd " (quote-sh suite) " && exec env PYTHONDONTWRITEBYTECODE=1 python3 esctest.py --expected-terminal=xterm "
                             (quote-sh (string-append "--include=" selection)) " "
                             (quote-sh (string-append "--logfile=" logfile)) " --no-print-logs"))
   (keys "Enter")
   (await-frame "Exit 0" 180)
   ;; The Python runner can exit zero even on failures. Check the report.
   (let* ((log (file->string logfile))
          (summary (find (lambda (line) (and (string-prefix? "*** " line) (string-contains line "known bug")))
                         (reverse (string-split log "\n")))))
     (unless (equal? summary expected) (display log) (error "VT compatibility failed" summary))
     (display summary) (newline)))
 (lambda ()
   (guard (e (#t #f)) (tmux "kill-server"))
   (delete-path dir #t)))
