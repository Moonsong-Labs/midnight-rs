(analyzed-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (calculate_square . %calculate_square.0))
  (contract-types
    (tcontract
      Calculator
      (get_square
        #f
        ((tfield (field-native)))
        (tfield (field-native)))
      (get_cube
        #f
        ((tfield (field-native)))
        (tfield (field-native)))))
  (kernel-declaration (%kernel.3 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%calc.2
        (0)
        (exported #f)
        (__compact_Cell
          (tcontract
            Calculator
            (get_square
              #f
              ((tfield (field-native)))
              (tfield (field-native)))
            (get_cube
              #f
              ((tfield (field-native)))
              (tfield (field-native)))))))
    (constructor
      ((%c.4
         (tcontract
           Calculator
           (get_square
             #f
             ((tfield (field-native)))
             (tfield (field-native)))
           (get_cube
             #f
             ((tfield (field-native)))
             (tfield (field-native))))))
      (seq (public-ledger %calc.2 (0) write (ttuple)
             (instructions
               (push (storage #f) (value (state-value cell (align 0 1))))
               (push
                 (storage #t)
                 (value (state-value cell (var-ref %c.4))))
               (ins (cached #f) (n 1)))
             (var-ref %c.4))
           (return (tuple)))))
  (circuit %calculate_square.0 (exported #t) (pure #f) (proof #t)
    ((%i.1 (tfield (field-native)))) (tfield (field-native))
    (return
      (contract-call
        get_square
        ((public-ledger %calc.2 (0) read
           (tcontract
             Calculator
             (get_square
               #f
               ((tfield (field-native)))
               (tfield (field-native)))
             (get_cube
               #f
               ((tfield (field-native)))
               (tfield (field-native))))
           (instructions
             (dup (n 0))
             (idx (cached #f) (pushPath #f) (path ((align 0 1))))
             (popeq (cached #f) (result (void)))))
          (tcontract
            Calculator
            (get_square
              #f
              ((tfield (field-native)))
              (tfield (field-native)))
            (get_cube
              #f
              ((tfield (field-native)))
              (tfield (field-native)))))
        (var-ref %i.1)))))
