(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (instance . %instance.18) (message . %message.19)
    (post . %post.16) (poster . %poster.17)
    (public_key . %public_key.14) (state . %state.15)
    (take_down . %take_down.13))
  (contract-types)
  (kernel-declaration (%kernel.33 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%state.15
        (0)
        (exported #t)
        (__compact_Cell (tenum STATE vacant occupied)))
      (%message.19
        (1)
        (exported #t)
        (__compact_Cell
          (tstruct
            Maybe
            (is_some (tboolean))
            (value (topaque "string")))))
      (%instance.18 (2) (exported #t) (Counter))
      (%poster.17 (3) (exported #t) (__compact_Cell (tbytes 32))))
    (constructor
      ()
      (seq (public-ledger %state.15 write (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value
                   (state-value
                     cell
                     (enum-ref (tenum STATE vacant occupied) vacant))))
               (ins (cached #f) (n 1)))
             (enum-ref (tenum STATE vacant occupied) vacant))
           (let* (((%tmp.35
                     (tstruct
                       Maybe
                       (is_some (tboolean))
                       (value (topaque "string")))) (call %none.21)))
             (public-ledger %message.19 write (1) write (ttuple)
               (instructions
                 (push (storage #f) (value (state-value cell (align 1 1))))
                 (push
                   (storage #t)
                   (value (state-value cell (var-ref %tmp.35))))
                 (ins (cached #f) (n 1)))
               (var-ref %tmp.35)))
           (let* (((%tmp.34 (tunsigned 65535)) (safe-cast
                                                 (tunsigned 65535)
                                                 (tunsigned 1)
                                                 '1)))
             (public-ledger %instance.18 update (2) increment (ttuple)
               (instructions
                 (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                 (addi (immediate (value->int (var-ref %tmp.34))))
                 (ins (cached #t) (n 1)))
               (var-ref %tmp.34)))
           (return (tuple)))))
  (export-typedef STATE () (tenum STATE vacant occupied))
  (circuit %some.28 (exported #f) (pure #t) (proof #f)
    ((%value.32 (topaque "string")))
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (topaque "string")))
    (return
      (new (tstruct
             Maybe
             (is_some (tboolean))
             (value (topaque "string")))
           '#t
           (var-ref %value.32))))
  (circuit %none.21 (exported #f) (pure #t) (proof #f) ()
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (topaque "string")))
    (return
      (new (tstruct
             Maybe
             (is_some (tboolean))
             (value (topaque "string")))
           '#f
           (default (topaque "string")))))
  (native %persistentHash.25
    (entry "__compactRuntime.persistentHash" circuit)
    (type-arguments (tvector 3 (tbytes 32)))
    ((%value.36 (tvector 3 (tbytes 32)))) (tbytes 32))
  (witness %local_secret_key.24 () (tbytes 32))
  (circuit %post.16 (exported #t) (pure #f) (proof #t)
    ((%new_message.29 (topaque "string"))) (ttuple)
    (seq (assert
           (== (tenum STATE vacant occupied)
               (public-ledger %state.15 read (0) read (tenum STATE vacant occupied)
                 (instructions
                   (dup (n 0))
                   (idx (cached #f) (pushPath #f) (path ((align 0 1))))
                   (popeq (cached #f) (result (void)))))
               (enum-ref (tenum STATE vacant occupied) vacant))
           "Attempted to post to an occupied board")
         (let* (((%tmp.31 (tbytes 32)) (call
                                         %public_key.14
                                         (call %local_secret_key.24)
                                         (field->bytes
                                           32
                                           (field-native)
                                           (safe-cast
                                             (tfield (field-native))
                                             (tunsigned
                                               18446744073709551615)
                                             (public-ledger %instance.18 read (2) read
                                               (tunsigned
                                                 18446744073709551615)
                                               (instructions
                                                 (dup (n 0))
                                                 (idx (cached #f)
                                                      (pushPath #f)
                                                      (path ((align 2 1))))
                                                 (popeq
                                                   (cached #t)
                                                   (result (void))))))))))
           (public-ledger %poster.17 write (3) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 3 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tmp.31))))
               (ins (cached #f) (n 1)))
             (var-ref %tmp.31)))
         (let* (((%tmp.30
                   (tstruct
                     Maybe
                     (is_some (tboolean))
                     (value (topaque "string")))) (call
                                                    %some.28
                                                    (var-ref
                                                      %new_message.29))))
           (public-ledger %message.19 write (1) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 1 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %tmp.30))))
               (ins (cached #f) (n 1)))
             (var-ref %tmp.30)))
         (public-ledger %state.15 write (0) write (ttuple)
           (instructions
             (push (storage #f) (value (state-value cell (align 0 1))))
             (push
               (storage #t)
               (value
                 (state-value
                   cell
                   (enum-ref (tenum STATE vacant occupied) occupied))))
             (ins (cached #f) (n 1)))
           (enum-ref (tenum STATE vacant occupied) occupied))
         (return (tuple))))
  (circuit %take_down.13 (exported #t) (pure #f) (proof #t) ()
    (topaque "string")
    (seq (assert
           (== (tenum STATE vacant occupied)
               (public-ledger %state.15 read (0) read (tenum STATE vacant occupied)
                 (instructions
                   (dup (n 0))
                   (idx (cached #f) (pushPath #f) (path ((align 0 1))))
                   (popeq (cached #f) (result (void)))))
               (enum-ref (tenum STATE vacant occupied) occupied))
           "Attempted to take down post from an empty board")
         (assert
           (== (tbytes 32)
               (public-ledger %poster.17 read (3) read (tbytes 32)
                 (instructions
                   (dup (n 0))
                   (idx (cached #f) (pushPath #f) (path ((align 3 1))))
                   (popeq (cached #f) (result (void)))))
               (call
                 %public_key.14
                 (call %local_secret_key.24)
                 (field->bytes
                   32
                   (field-native)
                   (safe-cast
                     (tfield (field-native))
                     (tunsigned 18446744073709551615)
                     (public-ledger %instance.18 read (2) read
                       (tunsigned 18446744073709551615)
                       (instructions
                         (dup (n 0))
                         (idx (cached #f)
                              (pushPath #f)
                              (path ((align 2 1))))
                         (popeq (cached #t) (result (void)))))))))
           "Attempted to take down post, but not the current poster")
         (let* (((%former_msg.20 (topaque "string")) (elt-ref
                                                       (public-ledger %message.19 read
                                                         (1) read
                                                         (tstruct
                                                           Maybe
                                                           (is_some
                                                             (tboolean))
                                                           (value
                                                             (topaque
                                                               "string")))
                                                         (instructions
                                                           (dup (n 0))
                                                           (idx (cached #f)
                                                                (pushPath
                                                                  #f)
                                                                (path
                                                                  ((align
                                                                     1
                                                                     1))))
                                                           (popeq
                                                             (cached #f)
                                                             (result
                                                               (void)))))
                                                       value
                                                       1)))
           (seq (public-ledger %state.15 write (0) write (ttuple)
                  (instructions
                    (push
                      (storage #f)
                      (value (state-value cell (align 0 1))))
                    (push
                      (storage #t)
                      (value
                        (state-value
                          cell
                          (enum-ref
                            (tenum STATE vacant occupied)
                            vacant))))
                    (ins (cached #f) (n 1)))
                  (enum-ref (tenum STATE vacant occupied) vacant))
                (let* (((%tmp.23 (tunsigned 65535)) (safe-cast
                                                      (tunsigned 65535)
                                                      (tunsigned 1)
                                                      '1)))
                  (public-ledger %instance.18 update (2) increment (ttuple)
                    (instructions
                      (idx (cached #f) (pushPath #t) (path ((align 2 1))))
                      (addi (immediate (value->int (var-ref %tmp.23))))
                      (ins (cached #t) (n 1)))
                    (var-ref %tmp.23)))
                (let* (((%tmp.22
                          (tstruct
                            Maybe
                            (is_some (tboolean))
                            (value (topaque "string")))) (call %none.21)))
                  (public-ledger %message.19 write (1) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 1 1))))
                      (push
                        (storage #t)
                        (value (state-value cell (var-ref %tmp.22))))
                      (ins (cached #f) (n 1)))
                    (var-ref %tmp.22)))
                (return (var-ref %former_msg.20))))))
  (circuit %public_key.14 (exported #t) (pure #t) (proof #f)
    ((%sk.26 (tbytes 32)) (%instance.27 (tbytes 32)))
    (tbytes 32)
    (return
      (call
        %persistentHash.25
        (tuple
          (single
            '#vu8(98 98 111 97 114 100 58 112 107 58 0 0 0 0 0 0 0 0 0 0
                  0 0 0 0 0 0 0 0 0 0 0 0))
          (single (var-ref %instance.27))
          (single (var-ref %sk.26)))))))
