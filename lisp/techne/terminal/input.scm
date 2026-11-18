;;; The frontend sends canonical editor keys; encode xterm input for the
;;; modes selected by the child. An unencodable key returns #f.

(define (terminal-key t key)
  "Encode editor KEY per terminal T's input modes, or return #f."
  (let mods ((k key) (shift #f) (alt #f) (ctrl #f))
    (cond
      ((string-prefix? "S-" k) (mods (substring k 2) #t alt ctrl))
      ((string-prefix? "M-" k) (mods (substring k 2) shift #t ctrl))
      ((string-prefix? "C-" k) (mods (substring k 2) shift alt #t))
      (else
       (let* ((m (+ 1 (if shift 1 0) (if alt 2 0) (if ctrl 4 0)))
              (suffix (assoc k '(("<up>" . "A") ("<down>" . "B") ("<right>" . "C") ("<left>" . "D")
                                 ("<home>" . "H") ("<end>" . "F") ("<f1>" . "P") ("<f2>" . "Q")
                                 ("<f3>" . "R") ("<f4>" . "S"))))
              (tilde (assoc k '(("<insert>" . "2") ("<delete>" . "3") ("<prior>" . "5") ("<next>" . "6")
                                ("<pageup>" . "5") ("<pagedown>" . "6")
                                ("<f5>" . "15") ("<f6>" . "17") ("<f7>" . "18") ("<f8>" . "19")
                                ("<f9>" . "20") ("<f10>" . "21") ("<f11>" . "23") ("<f12>" . "24"))))
              (ordinary (cond ((string=? k "SPC") #\space) ((string=? k "TAB") #\tab)
                              ((string=? k "RET") #\return) ((string=? k "DEL") #\x7f)
                              ((string=? k "ESC") #\x1b) ((= (string-length k) 1) (string-ref k 0)) (else #f))))
         (cond
           (suffix (string-append (cond ((> m 1) (string-append csi "1;" (number->string m)))
                                        ((or (string-prefix? "<f" k) (terminal-mode? t 'application-cursor)) (string-append esc "O"))
                                        (else csi)) (cdr suffix)))
           (tilde (string-append csi (cdr tilde) (if (= m 1) "" (string-append ";" (number->string m))) "~"))
           ((or (string=? k "<backtab>") (and shift (string=? k "TAB")))
            (string-append (if alt esc "") csi "Z"))
           (ordinary
            (let* ((c (if shift (char-upcase ordinary) ordinary)) (n (char->integer (char-upcase c)))
                   (text (cond ((not ctrl) (if (and (char=? c #\return) (terminal-mode? t 'newline)) "\r\n" (string c)))
                               ((or (= n 32) (= n 50)) (string #\x00))
                               ((<= 64 n 95) (string (integer->char (- n 64))))
                               ((or (= n 63) (= n 56)) (string #\x7f))
                               ((memv n '(9 13 27 127)) (string c))
                               (else #f))))
              (and text (string-append (if alt esc "") text))))
           (else #f)))))))

(define (terminal-paste t text)
  "Encode TEXT for pasting into terminal T, filtering control characters."
  ;; Normalize line endings and don't let clipboard controls inject a
  ;; bracketed-paste terminator or make a terminal reply of their own.
  (let ((text (call-with-output-string
               (lambda (p)
                 (let loop ((i 0))
                   (when (< i (string-length text))
                     (let* ((c (string-ref text i)) (n (char->integer c))
                            (crlf (and (= n 13) (< (+ i 1) (string-length text))
                                       (char=? (string-ref text (+ i 1)) #\newline))))
                       (cond ((or (= n 10) (= n 13)) (write-char #\return p))
                             ((or (= n 9) (and (>= n 32) (not (<= 127 n 159)))) (write-char c p)))
                       (loop (+ i (if crlf 2 1))))))))))
    (if (terminal-mode? t 'bracketed-paste) (string-append csi "200~" text csi "201~") text)))
