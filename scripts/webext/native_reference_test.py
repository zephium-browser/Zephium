import copy
import unittest
from native_reference import matching_evaluations, reference_for


class ReferenceTests(unittest.TestCase):
    def setUp(self):
        self.extension = 'ghmbeldphafepmbegfdlkpapadhbakde'
        self.reference = reference_for(self.extension)
        source = 'chrome-extension://' + self.extension + '/client.js'
        self.rows = [dict(extension=self.extension, context='ContentScript', process=2,
            event=dict(kind='native-evaluation', source=source, success=success,
                message='' if success else self.reference['repeatedEvaluation']['message'],
                stack='' if success else source + ':238:2936',
                scope=dict(page=1, frame=2, document=3, world='isolated', trigger='executeScript', requestId=index)))
            for index, success in enumerate([True, False], 1)]

    def test_exact_second_evaluation_matches_chromium(self):
        self.assertEqual(matching_evaluations(self.rows, self.reference), {1})

    def test_other_errors_worlds_and_first_evaluations_stay_failures(self):
        for key, value in [('message', 'another error'), ('source', 'other.js'), ('stack', 'other.js:238:1')]:
            rows = copy.deepcopy(self.rows)
            rows[1]['event'][key] = value
            self.assertFalse(matching_evaluations(rows, self.reference))
        for key, value in [('world', ''), ('world', 'main'), ('document', 4), ('requestId', 1), ('trigger', 'manifest')]:
            rows = copy.deepcopy(self.rows)
            rows[1]['event']['scope'][key] = value
            self.assertFalse(matching_evaluations(rows, self.reference))
        self.assertFalse(matching_evaluations(self.rows[1:], self.reference))
        self.assertFalse(matching_evaluations(self.rows, reference_for('unknown')))
        rows = copy.deepcopy(self.rows)
        rows[0]['event']['success'] = False
        self.assertFalse(matching_evaluations(rows, self.reference))

    def test_third_evaluation_is_not_whitelisted(self):
        rows = copy.deepcopy(self.rows + [self.rows[1]])
        rows[2]['event']['scope']['requestId'] = 3
        self.assertEqual(matching_evaluations(rows, self.reference), {1})

    def test_prior_success_must_have_the_same_authority_and_valid_request(self):
        for value in [None, True, 0, -1, '1']:
            rows = copy.deepcopy(self.rows)
            rows[0]['event']['scope']['requestId'] = value
            self.assertFalse(matching_evaluations(rows, self.reference))
        for field, value in [('extension', 'foreign'), ('context', 'Popup')]:
            rows = copy.deepcopy(self.rows)
            rows[0][field] = value
            self.assertFalse(matching_evaluations(rows, self.reference))


if __name__ == '__main__':
    unittest.main()
