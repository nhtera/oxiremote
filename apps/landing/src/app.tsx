import SiteNav from './sections/site-nav'
import Hero from './sections/hero'
import TrustStrip from './sections/trust-strip'
import FeatureGrid from './sections/feature-grid'
import HowItWorks from './sections/how-it-works'
import CompareTable from './sections/compare-table'
import InstallBlock from './sections/install-block'
import FaqBlock from './sections/faq-block'
import FinalCta from './sections/final-cta'
import SiteFooter from './sections/site-footer'
import useScrollReveal from './components/use-scroll-reveal'

export default function App() {
  useScrollReveal()
  return (
    <>
      <SiteNav />
      <main className="relative">
        <Hero />
        <TrustStrip />
        <FeatureGrid />
        <HowItWorks />
        <CompareTable />
        <InstallBlock />
        <FaqBlock />
        <FinalCta />
      </main>
      <SiteFooter />
    </>
  )
}
